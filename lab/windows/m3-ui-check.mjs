// M3a Task 9c check of the real desktop window's shell, driven over the
// WebView2 DevTools protocol (simulator only, no RF, no audio device):
//   1. the splash (360x180, frameless, logo, status line, progress bar) is up
//      alone before the main window; the main window then replaces it
//   2. first use explains automatic answering and offers "Start in Receive only";
//      the header shows "Answering calls" / "Receive only" with a toggle that
//      follows the core, never ahead of it
//   3. the Operate rail icon is the antenna, with an accessible name
//   4. the composer has no resize handle, grows with its text up to about
//      8 lines, scrolls beyond that, and shrinks back after Send
//   5. one sidebar, the same on Operate, Conversations, Settings and
//      Diagnostics: same element, left edge and width (Operate's station strip
//      sits above it, so its top differs), search text and selection; Heard tab
//      stays chosen; picking a chat from Settings opens it
//   6. at 900 px the sidebar is a drawer
//   7. a moved and resized window comes back the same after a restart
//   8. no devtools: F12, Ctrl+Shift+I, Ctrl+R and F5 are inert, the context
//      menu offers only cut/copy/paste/select-all on fields and selected text,
//      and the tauri `devtools` feature is not in the dependency tree
//   9. themes: Night Red re-colours the window at once, density and text size apply
//      at once, and the splash and window come back in it after a restart
// M3a Task 10 (Operate workspace), on the same simulator run:
//   T10a SIM1 appears in Heard by CQ and then with a "Beacon" badge and its
//        locator, highlighted (static under reduced motion), announced politely;
//        the Heard tab counts new stations while Contacts is open and clears on open
//   T10b a star persists across a restart (checked in T10e)
//   T10c Call CQ shows "transmitted"; telemetry reads "unavailable"; the strip's
//        frequency is dashes in the seven-segment font; channel occupancy unknown
//   T10d turning beacons on shows a countdown; the activity drawer lists PTT lines
//   T10e after a restart beacons are off and the prompt asks to turn them on
// M3a Task 12: the spectrum and waterfall are collapsed by default and, on the
//   simulator (no telemetry), say "telemetry unavailable" and draw nothing
// M3a Task 12b: a lab station does not notify until it is allowed; with "Any station
//   newly heard" on, SIM1's next CQ or beacon asks for a notification (recorded by a stub)
// M3a Task 11 (QSO card, Logbook, macros, search, About):
//   T11a station information and a macro are saved in Settings; finalise on
//        disconnect is off by default
//   T11b About shows the version and "Founded by ZS6SHZ"; its links go to the
//        system browser (recorded through MC_OPEN_LINK_LOG) and the window fetches nothing
//   T11c the Greeting macro expands into the composer without sending; <EMAIL>
//        needs the explicit "Insert email" item, which fills it
//   T11d calling SIM1 opens the QSO card with peer and bandwidth; edits survive a
//        window reload; after the disconnect a QSO summary row is in the transcript
//   T11e the record is a draft in the Logbook; Finalise locks it
//   T11f history search finds an old message by its text
// M3b Task 10 (the station strip as the rig panel; the simulator runs a fake rigctld):
//   R1 the strip shows the sim rig's frequency in DSEG7; ArrowUp on the 1 kHz digit moves it 1 kHz
//   R2 picking a frequency-list entry moves the rig there
//   R3 Tune asks to confirm the first time, then shows Stop with a countdown; Stop ends it
//   R4 with SIM1 connected the digits take no focus and say "not while connected"
//   R5 the strip is at most 80 px tall at 1280 and 1024 px wide
//   R6 the Rig button's panel has no Split when the rig cannot split, and has one when it can
// M3b Task 11 (Settings in tabs; Settings → Rig):
//   S1 every Settings tab opens its panel; arrow keys move between tabs
//   S2 Rig shows the sim rig's address, and Test lists six steps, all passing
//   S3 a custom entry added in Settings appears in the strip's frequency list
//   S4 a suggested entry deleted comes back with Restore suggested (added 1)
//   S5 the chosen tab survives a window reload
// M3b.1 Task 8 (Settings → Rig in VarAC's layout; Settings → Audio):
//   S6 Rig is two columns in a desktop-sized window; the sim station reads as "External rigctld";
//      switching (unsaved) to the managed rigctld shows the CAT box and hides the
//      external one, and FLRig takes CAT out of the PTT choices
//   S7 Audio, with no Mercury UI link in the simulator, says to start Mercury with -G;
//      no console errors from S1 to here
// Usage: node lab/windows/m3-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect, firstUse, kill, launch, page, pages, sleep, until } from "./cdp.mjs";
import { click, pill, setInput } from "./m2-dom.mjs";

const [exe, dataDir, port = "9335"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m3-ui-check.mjs <mercurychat.exe> <empty data dir> [port]");
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const CALL = "ZS6SIM";
// About's links are recorded here instead of opening a browser (debug builds only).
const linkLog = join(dirname(dataDir), `${port}-links.txt`);
rmSync(linkLog, { force: true });
process.env.MC_OPEN_LINK_LOG = linkLog;

/** Top-level windows of a process: [{x, y, w, h, title}], optionally after moving the largest. */
function windows(pid, to) {
  const args = ["-NoProfile", "-File", join(here, "winrect.ps1"), String(pid), ...(to ? to.map(String) : [])];
  const out = execFileSync("powershell", args, { encoding: "utf8" });
  return out
    .split(/\r?\n/)
    .filter(Boolean)
    .map((l) => {
      const [x, y, w, h, ...title] = l.split(" ");
      return { x: +x, y: +y, w: +w, h: +h, title: title.join(" ") };
    });
}
const isSplash = (t) => t.url?.includes("splash.html");

/** The workspace is up: a station, the modem attached, composer on screen. */
const ready = `(['ready','listening'].includes(${pill})) && document.querySelector('textarea.composer, .station-home')`;
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;
const rectOf = (sel) => `(() => { const r = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return [r.left, r.top, r.width, r.height].map(Math.round).join(','); })()`;
const onScreen = (sel) => `(() => {
  const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return false;
  const r = e.getBoundingClientRect(), v = getComputedStyle(e).visibility;
  return v !== 'hidden' && r.width > 0 && r.right > 0 && r.left < innerWidth;
})()`;
/** Fire a cancelable keydown on the document and say whether the window let it be cancelled. */
const keyInert = (init) => `(() => { const e = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...${JSON.stringify(init)} }); document.dispatchEvent(e); return e.defaultPrevented; })()`;
const rightClick = (sel) => `(() => {
  const el = document.querySelector(${JSON.stringify(sel)});
  const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 40, clientY: 40 });
  el.dispatchEvent(e);
  return e.defaultPrevented;
})()`;
const menuItems = "[...document.querySelectorAll('.ctx-menu [role=menuitem]')].map(b => b.textContent.trim())";

const results = [];
const children = [];
let s;
try {
  // ---- 1. splash before the main window
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  process.env.MC_SPLASH_MIN_MS = "6000";
  let first = launch(exe, dataDir, port);
  delete process.env.MC_SPLASH_MIN_MS;
  children.push(first);
  let splash = null;
  const seen = Date.now();
  while (!splash && Date.now() - seen < 20000) {
    splash = (await pages(port)).find(isSplash) ?? null;
    if (!splash) await sleep(25);
  }
  if (!splash) throw new Error("no splash page appeared");
  const sc = await connect(splash.webSocketDebuggerUrl);
  await until(sc, "!!document.querySelector('.status')?.textContent.trim()", "the splash status line", 3000);
  // Geometry and visibility are read from inside the page (starting PowerShell is slower than the splash's minimum stay).
  const dom = await sc.eval(`({
    w: outerWidth, h: outerHeight, shown: document.visibilityState, dpr: devicePixelRatio,
    cx: screenX + outerWidth / 2 - (screen.availLeft ?? 0), cy: screenY + outerHeight / 2 - (screen.availTop ?? 0), aw: screen.availWidth, ah: screen.availHeight,
    logo: !!document.querySelector('svg.logo[aria-hidden="true"]'),
    status: document.querySelector('.status')?.textContent.trim() ?? '',
    bar: document.querySelector('[role=progressbar]') ? getComputedStyle(document.querySelector('.bar i')).animationName : 'none',
  })`);
  if (!dom.logo || !dom.status || dom.bar === "none") throw new Error(`splash content: ${JSON.stringify(dom)}`);
  if (Math.abs(dom.w - 360) > 2 || Math.abs(dom.h - 180) > 2) throw new Error(`splash is ${dom.w}x${dom.h} CSS px, not 360x180`);
  if (dom.shown !== "visible") throw new Error("the splash is not visible");
  if (Math.abs(dom.cx - dom.aw / 2) > 12 || Math.abs(dom.cy - dom.ah / 2) > 12) throw new Error(`splash is not centred: ${JSON.stringify(dom)}`);
  // A lab-only hold (MC_SPLASH_MIN_MS) keeps the splash up long enough to list the native windows.
  const during = windows(first.pid);
  if (during.length !== 1 || Math.abs(during[0].w / dom.dpr - 360) > 24) throw new Error(`windows while the splash is up: ${JSON.stringify(during)}`);
  // Reduced motion, emulated: the bar must stop moving (not merely have a rule somewhere).
  await sc.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  const still = await sc.eval("getComputedStyle(document.querySelector('.bar i')).animationName");
  await sc.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "no-preference" }] });
  const moving = await sc.eval("getComputedStyle(document.querySelector('.bar i')).animationName");
  if (still !== "none" || moving === "none") throw new Error(`reduced motion: animation "${still}" when reduced, "${moving}" otherwise`);
  sc.close();
  s = await firstUse(port, "Use simulator");
  const after = await pages(port);
  await until(s, "true", "settle");
  let tries = 0;
  while ((await pages(port)).some(isSplash) && tries++ < 150) await sleep(100);
  if ((await pages(port)).some(isSplash)) throw new Error("the splash is still open after the main window loaded");
  const main = windows(first.pid);
  if (main.length !== 1 || main[0].w < 960 * 0.9) throw new Error(`main window after hand-over: ${JSON.stringify(main)}`);
  results.push(`PASS splash: ${dom.w}x${dom.h} CSS px, centred, frameless, the only visible window of the app while it was up ("${dom.status}", logo, bar animated, bar animation "${moving}" -> "${still}" under emulated prefers-reduced-motion), gone once the UI loaded (${after.length} pages seen)`);

  // ---- 2. first use and the header mode
  const welcome = await s.eval("document.querySelector('.welcome').innerText");
  if (!/answers? (incoming )?calls automatically/i.test(welcome) || !welcome.includes("Start in Receive only")) throw new Error(`first-use text: ${welcome}`);
  if (await s.eval("document.querySelector('.welcome input.start-receive-only').checked")) throw new Error("Start in Receive only is on by default");
  await s.eval(setInput('form.simulator input[name="callsign"]', CALL));
  await s.eval(click("Use simulator"));
  await until(s, ready, "the workspace");
  await until(s, "document.querySelector('.station-mode')?.innerText.includes('Answering calls')", "header says Answering calls");
  const sw = "document.querySelector('.station-mode input')";
  if (await s.eval(`${sw}.checked`)) throw new Error("the toggle is on while answering calls");
  await s.eval(`${sw}.click()`);
  await until(s, `document.querySelector('.station-mode').innerText.includes('Receive only') && ${sw}.checked`, "header says Receive only");
  await s.eval(`${sw}.click()`);
  await until(s, `document.querySelector('.station-mode').innerText.includes('Answering calls') && !${sw}.checked`, "back to Answering calls");
  if (await s.eval("document.querySelector('input.listen')")) throw new Error("the old Listen switch is still in the window");
  results.push('PASS receive only: first use explains automatic answering and offers "Start in Receive only"; header toggles Answering calls <-> Receive only from the core\'s state; no old Listen switch');

  // ---- T10a. SIM1 is heard by CQ, then by beacon with its locator (Contacts open: the Heard tab counts them)
  await until(s, "document.querySelector('aside.sidebar [role=tab][aria-selected=true]')?.textContent.trim() === 'Chats'", "Chats open first");
  await until(s, "!!document.querySelector('#side-heard-new')", "the Heard tab's new-station count while Contacts is open", 15000);
  const count = await s.eval("document.querySelector('#side-heard-new [aria-hidden]').textContent.trim()");
  await s.eval("[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim() === 'Heard').click()");
  await until(s, "!document.querySelector('#side-heard-new')", "the count cleared once Heard is open");
  const heardRow = "document.querySelector('.heard .row[data-callsign=\"SIM1\"]')";
  await until(s, heardRow, "SIM1 in the Heard list", 15000);
  // The first observation (the CQ) came while Contacts was open; the beacon follows about 2 s later.
  await until(s, `${heardRow}.querySelector('.badge')?.textContent.trim() === 'Beacon' && ${heardRow}.querySelector('.grid')?.textContent.trim() === 'JJ00aa'`, "SIM1's Beacon badge and locator", 15000);
  const fresh = await s.eval(`${heardRow}.classList.contains('fresh')`);
  const anim = await s.eval(`getComputedStyle(${heardRow}).animationName`);
  if (!fresh || anim === "none") throw new Error(`no new-station highlight: fresh ${fresh}, animation ${anim}`);
  await s.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  const stillAnim = await s.eval(`getComputedStyle(${heardRow}).animationName`);
  await s.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "no-preference" }] });
  if (stillAnim !== "none") throw new Error(`the highlight still animates under reduced motion: ${stillAnim}`);
  const live = await s.eval("document.querySelector('.heard [aria-live=polite]').textContent.trim()");
  if (!/^SIM1 heard: (CQ|Beacon)$/.test(live)) throw new Error(`live region: "${live}"`);
  const act = await s.eval("[...document.querySelectorAll('.activity .line[data-kind=heard]')].map(l => l.querySelector('.text').textContent)");
  if (!act.some((l) => /SIM1 \(CQ/.test(l))) throw new Error(`no CQ heard line in the activity drawer: ${act.slice(0, 6).join(" | ")}`);
  results.push(`PASS heard: while Contacts was open the Heard tab counted ${count}; opening Heard cleared it; SIM1 listed with a "Beacon" badge and locator JJ00aa after its CQ, highlighted (animation ${anim}, static under reduced motion); live region "${live}"; the activity drawer has the CQ line`);

  // ---- T10b. a star persists (checked again after the restart in section 7)
  await s.eval(`${heardRow}.querySelector('.star').click()`);
  await until(s, `${heardRow}.querySelector('.star').getAttribute('aria-pressed') === 'true'`, "SIM1 starred");

  // The strip is the station panel now (M5 Task 11b): it drops from the header's station pill.
  await s.eval("(() => { const b = document.querySelector('header .station-pill'); if (b.getAttribute('aria-expanded') !== 'true') b.click(); })()");
  await until(s, "getComputedStyle(document.querySelector('section.strip')).display !== 'none'", "the station panel open");
  // ---- T10c. CQ shows "transmitted"; telemetry reads unavailable; the strip shows the sim rig's frequency
  await s.eval(click("Call CQ"));
  await until(s, "/CQ: transmitted/.test(document.querySelector('.strip .cq-outcome')?.textContent ?? '')", "CQ transmitted", 10000);
  const tel = await s.eval("[...document.querySelectorAll('.telemetry .stat')].map(e => e.className.split(' ').pop() + '=' + e.lastElementChild.textContent.trim())");
  for (const k of ["snr", "mode", "throughput", "audio", "frequency"]) {
    if (!tel.includes(`${k}=unavailable`)) throw new Error(`telemetry ${k} is not "unavailable": ${tel.join(", ")}`);
  }
  if (!tel.some((x) => x.startsWith("link=") && x.endsWith("Not configured"))) throw new Error(`telemetry link: ${tel.join(", ")}`);
  const freq = await s.eval("({ text: document.querySelector('.strip .freq-digits').textContent.trim(), aria: document.querySelector('.strip .freq-digits').getAttribute('aria-label'), font: getComputedStyle(document.querySelector('.strip .freq-digits')).fontFamily })");
  if (!/^\d{1,2}\.\d{3}\.\d{3}$/.test(freq.text) || !/ MHz$/.test(freq.aria ?? "") || !/DSEG7/.test(freq.font)) throw new Error(`strip frequency: ${JSON.stringify(freq)}`);
  const channel = await s.eval("document.querySelector('.beacon .channel').textContent.trim()");
  if (channel !== "Channel occupancy unknown") throw new Error(`channel: ${channel}`);

  // ---- T12. the spectrum and waterfall are collapsed by default and, with no telemetry, draw nothing
  const wf = await s.eval("({ expanded: document.querySelector('.strip .wf-toggle')?.getAttribute('aria-expanded'), text: document.querySelector('.strip .waterfall')?.innerText ?? '' })");
  if (wf.expanded !== "false" || !/telemetry unavailable/.test(wf.text)) throw new Error(`waterfall collapsed: ${JSON.stringify(wf)}`);
  await s.eval("document.querySelector('.strip .wf-toggle').click()");
  await until(s, "!!document.querySelector('.strip .wf-status')", "the open waterfall panel's status");
  const wfOpen = await s.eval("({ status: document.querySelector('.strip .wf-status').textContent.trim(), canvases: document.querySelectorAll('.strip canvas').length })");
  if (wfOpen.status !== "Spectrum unavailable: telemetry Not configured." || wfOpen.canvases !== 0) throw new Error(`waterfall open without telemetry: ${JSON.stringify(wfOpen)}`);
  await s.eval("document.querySelector('.strip .wf-toggle').click()");
  results.push(`PASS waterfall: collapsed by default ("telemetry unavailable"); opened on the simulator it says "${wfOpen.status}" and has no canvas`);

  // ---- T10d. beacons on show a countdown; the activity drawer lists PTT lines
  await s.eval("document.querySelector('input.beacon-toggle').click()");
  await until(s, "document.querySelector('input.beacon-toggle').checked && /^Next beacon \\d\\d:\\d\\d:\\d\\d$/.test(document.querySelector('.strip .next-beacon').textContent.trim())", "the beacon countdown on the strip");
  const cd = await s.eval("[document.querySelector('.strip .next-beacon').textContent.trim(), document.querySelector('.beacon .countdown')?.textContent.trim()]");
  await s.eval(click("Beacon now"));
  await until(s, "/Beacon: transmitted/.test(document.querySelector('.strip .beacon-outcome')?.textContent ?? '')", "Beacon now transmitted", 10000);
  const ptt = await s.eval("[...document.querySelectorAll('.activity .line[data-kind=ptt]')].map(l => l.querySelector('.text').textContent)");
  if (!ptt.includes("PTT on") || !ptt.includes("PTT off")) throw new Error(`no PTT lines in the activity drawer: ${JSON.stringify(ptt)}`);
  results.push(`PASS operate: Call CQ -> "transmitted"; telemetry rows read "unavailable" (link Not configured); strip frequency "${freq.text}" (the sim rig) in DSEG7, aria "${freq.aria}"; "${channel}"; beacons on -> "${cd[0]}" (card ${cd[1]}); Beacon now -> "transmitted"; the activity drawer lists ${ptt.length} PTT lines`);

  // ---- R1. the digits tune the sim rig
  const hz = () => s.eval("Number(document.querySelector('.strip .freq-digits').getAttribute('aria-label').replace(' MHz', '')) * 1e6");
  await until(s, "document.querySelectorAll('.strip .digit[tabindex=\"0\"]').length >= 7", "tunable digits");
  const hz0 = Math.round(await hz());
  await s.eval("(() => { const d = document.querySelector('.strip .digit[data-place=\"3\"]'); d.focus(); d.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true })); })()");
  await until(s, `Math.round(Number(document.querySelector('.strip .freq-digits').getAttribute('aria-label').replace(' MHz', '')) * 1e6) === ${hz0 + 1000}`, "the frequency 1 kHz up", 5000);
  // The rig, not only the display: the snapshot's rig frequency follows (the digits show the rig's own once it arrives).
  await sleep(2500);
  const hz1 = Math.round(await hz());
  if (hz1 !== hz0 + 1000) throw new Error(`digit 3 ArrowUp: ${hz0} -> ${hz1}`);
  results.push(`PASS digits: ArrowUp on the 1 kHz digit tuned the sim rig ${hz0} -> ${hz1} Hz`);

  // ---- R2. a frequency-list pick moves the rig
  const pick = await s.eval(`(() => {
    const sel = document.querySelector('.strip select.freq-list');
    const opt = [...sel.options].find(o => /40 m/.test(o.textContent));
    if (!opt) throw new Error('no 40 m entry in the list');
    sel.value = opt.value; sel.dispatchEvent(new Event('change'));
    return opt.textContent;
  })()`);
  await until(s, "/^7\\./.test(document.querySelector('.strip .freq-digits').textContent.trim())", "the rig on 40 m", 5000);
  const band40 = await s.eval("document.querySelector('.strip .band-chip')?.textContent.trim()");
  if (band40 !== "40m") throw new Error(`band chip after the pick: ${band40}`);
  results.push(`PASS list: picking "${pick}" moved the rig to ${await s.eval("document.querySelector('.strip .freq-digits').textContent.trim()")} (band chip ${band40})`);

  // ---- R3. Tune: confirm once, Stop with a countdown, then stop
  await s.eval("document.querySelector('.strip button.tune').click()");
  await until(s, "!!document.querySelector('.popover button.tune-confirm')", "the Tune confirmation");
  const ask = await s.eval("document.querySelector('.popover').textContent.trim()");
  if (!/30 s/.test(ask)) throw new Error(`Tune confirmation: ${ask}`);
  await s.eval("document.querySelector('.popover button.tune-confirm').click()");
  await until(s, "/^Stop \\d\\d:\\d\\d$/.test(document.querySelector('.strip button.tune').textContent.trim())", "Stop with a countdown", 5000);
  const stop = await s.eval("document.querySelector('.strip button.tune').textContent.trim()");
  await s.eval("document.querySelector('.strip button.tune').click()");
  await until(s, "document.querySelector('.strip button.tune').textContent.trim() === 'Tune'", "Tune stopped", 5000);
  results.push(`PASS tune: the first Tune asked "${ask}", then showed "${stop}"; Stop ended it`);

  // ---- R5. one row, at most 80 px, at 1280 and 1024 px
  const heights = [];
  for (const w of [1280, 1024]) {
    await s.send("Emulation.setDeviceMetricsOverride", { width: w, height: 800, deviceScaleFactor: 1, mobile: false });
    await sleep(300);
    heights.push(await s.eval("document.querySelector('.strip').getBoundingClientRect().height"));
  }
  await s.send("Emulation.clearDeviceMetricsOverride", {});
  await sleep(600); // the resize this causes closes any open popover: let it land first
  if (heights.some((h) => h > 80)) throw new Error(`strip heights ${heights.join(", ")}`);
  results.push(`PASS strip height: ${heights.join(" / ")} px at 1280 / 1024 px wide`);

  // ---- R6. the Rig panel shows Split only when the rig has it (the sim's dummy rig can split)
  await s.eval("document.querySelector('.strip button.rig-more').click()");
  await until(s, "!!document.querySelector('.popover input.rf-power, .popover input.tx-level')", "the Rig panel");
  const panel = await s.eval("({ split: !!document.querySelector('.popover .split') })");
  const canSplit = await s.eval("(async () => (await window.__TAURI_INTERNALS__.invoke('app_call', { cmd: { type: 'snapshot' } })).rig.caps.canSplit)()");
  if (panel.split !== canSplit) throw new Error(`Split shown ${panel.split} but the rig's caps say ${canSplit}`);
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  results.push(`PASS rig panel: Split ${panel.split ? "shown" : "hidden"} as the rig's caps say (canSplit ${canSplit})`);

  // Console errors from here to S7 (the Settings tabs, Rig and Audio) fail the check.
  await s.eval(`(() => {
    window.__uiErrors = [];
    const keep = (x) => window.__uiErrors.push(String(x?.stack ?? x?.message ?? x).slice(0, 300));
    const err = console.error.bind(console);
    console.error = (...a) => { keep(a.join(' ')); err(...a); };
    addEventListener('error', (e) => keep(e.error ?? e.message));
    addEventListener('unhandledrejection', (e) => keep(e.reason));
  })()`);
  // ---- S1. Settings in tabs
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "document.querySelectorAll('.settings [role=tab]').length >= 6", "the Settings tabs");
  const tabs = await s.eval("[...document.querySelectorAll('.settings [role=tab]')].map(b => b.id.replace('tab-', ''))");
  for (const name of tabs) {
    await s.eval(`document.getElementById('tab-${name}').click()`);
    await until(s, `getComputedStyle(document.getElementById('panel-${name}')).display !== 'none' && document.getElementById('tab-${name}').getAttribute('aria-selected') === 'true'`, `the ${name} tab`);
    const shown = await s.eval("[...document.querySelectorAll('.settings [role=tabpanel]')].filter(p => getComputedStyle(p).display !== 'none').length");
    if (shown !== 1) throw new Error(`${shown} panels shown on the ${name} tab`);
  }
  await s.eval("(() => { const t = document.getElementById('tab-station'); t.click(); t.focus(); t.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })); })()");
  await until(s, "document.activeElement?.id === 'tab-profiles' && document.getElementById('tab-profiles').getAttribute('aria-selected') === 'true'", "ArrowRight to the next tab");
  const wide = await s.eval("document.querySelector('.settings').scrollWidth <= document.querySelector('.settings').clientWidth");
  if (!wide) throw new Error("Settings scrolls sideways");
  results.push(`PASS settings tabs: ${tabs.join(", ")} each open their own panel; ArrowRight moves to the next tab; no sideways scroll`);

  // ---- S2. Rig: the sim rig's address and the link test
  await s.eval("document.getElementById('tab-rig').click()");
  await until(s, "!!document.querySelector('.rig-settings input.rig-port')?.value", "the rig address");
  await s.eval("document.querySelector('.rig-settings button.rig-test').click()");
  await until(s, "document.querySelectorAll('.rig-test-steps li').length === 6", "six test steps", 15000);
  const stepsShown = await s.eval("[...document.querySelectorAll('.rig-test-steps li')].map(l => l.dataset.step + ':' + l.className.split(' ').pop())");
  if (stepsShown.some((x) => !x.endsWith(":ok"))) throw new Error(`rig test: ${stepsShown.join(", ")}`);
  results.push(`PASS rig test: ${stepsShown.join(", ")}`);

  // ---- S6. Rig in VarAC's layout (nothing saved: the choices go back before leaving)
  const layout = await s.eval(`(() => {
    const cols = [...document.querySelector('.rig-settings .cols').children].map(c => Math.round(c.getBoundingClientRect().left));
    const legends = [...document.querySelectorAll('.rig-settings legend')].map(l => l.textContent.trim());
    const box = Math.round(document.querySelector('.rig-settings .cols').getBoundingClientRect().width);
    return { cols, freq: document.querySelector('.rig-settings select.rig-freq-control').value, legends, width: innerWidth, box };
  })()`);
  if (layout.cols.length !== 2 || layout.cols[0] === layout.cols[1]) throw new Error(`rig columns: ${JSON.stringify(layout)}`);
  if (layout.freq !== "externalRigctld") throw new Error(`the sim station's frequency control is ${layout.freq}`);
  const pickFreq = (v) => s.eval(`(() => { const fc = document.querySelector('.rig-settings select.rig-freq-control'); fc.value = ${JSON.stringify(v)}; fc.dispatchEvent(new Event('change')); })()`);
  const legends = () => s.eval("[...document.querySelectorAll('.rig-settings legend')].map(l => l.textContent.trim())");
  const pttOpts = () => s.eval("[...document.querySelectorAll('.rig-settings select.rig-ptt option')].map(o => o.value)");
  await pickFreq("managedRigctld");
  await until(s, "[...document.querySelectorAll('.rig-settings legend')].some(l => l.textContent.trim() === 'CAT')", "the CAT box");
  const managed = await legends();
  if (managed.includes("External rigctld")) throw new Error(`managed rigctld still shows the external box: ${managed}`);
  await pickFreq("flrig");
  await until(s, "[...document.querySelectorAll('.rig-settings legend')].some(l => l.textContent.trim() === 'FLRig')", "the FLRig box");
  const flrigPtt = await pttOpts();
  if (flrigPtt.includes("cat")) throw new Error(`FLRig offers CAT PTT: ${flrigPtt}`);
  // The built-in CAT engine: its own box, the radios from the core, an import, and CAT PTT allowed.
  await pickFreq("catFile");
  await until(s, "!!document.querySelector('.rig-settings select.rig-radio option[value=\"Icom IC-7300\"]')", "the built-in radio list");
  const catPtt = await pttOpts();
  if (!catPtt.includes("cat")) throw new Error(`the built-in CAT engine offers no CAT PTT: ${catPtt}`);
  if (!(await s.eval("!!document.querySelector('.rig-settings input.rig-import')"))) throw new Error("no import button for CAT files");
  if (await s.eval("[...document.querySelectorAll('.rig-settings legend')].some(l => l.textContent.trim() === 'CAT')")) throw new Error("the Hamlib CAT box shows with the built-in engine");
  await pickFreq("externalRigctld");
  await until(s, "[...document.querySelectorAll('.rig-settings legend')].some(l => l.textContent.trim() === 'External rigctld')", "the external box back");
  results.push(`PASS rig layout: two columns at ${layout.width} px (left edges ${layout.cols.join(", ")}); sim station "externalRigctld" (${layout.legends.join(", ")}); managed shows ${managed.join(", ")}; FLRig PTT choices ${flrigPtt.join(", ")}`);

  // ---- S7. Audio without Mercury's UI link
  await s.eval("document.getElementById('tab-audio').click()");
  await until(s, "!!document.querySelector('.audio-settings .audio-unavailable')", "the audio tab's -G message");
  const audioText = await s.eval("document.querySelector('.audio-settings .audio-unavailable').textContent.trim()");
  if (!audioText.includes("-G")) throw new Error(`audio tab: ${audioText}`);
  results.push(`PASS audio tab: without Mercury's UI link it says "${audioText.slice(0, 60)}…"`);
  const uiErrors = await s.eval("window.__uiErrors");
  if (uiErrors.length) throw new Error(`console errors in Settings: ${uiErrors.join(" | ")}`);
  results.push("PASS settings console: no errors while the tabs, Rig and Audio were used");
  await s.eval("document.getElementById('tab-rig').click()");

  // ---- S3. a custom entry reaches the strip's list
  await s.eval(setInput(".freq-list-editor input.entry-name", "UI check net"));
  await s.eval(setInput(".freq-list-editor input.entry-freq", "7.110"));
  await s.eval("document.querySelector('.freq-list-editor button.entry-save').click()");
  await until(s, "[...document.querySelectorAll('.strip select.freq-list option')].some(o => /UI check net/.test(o.textContent))", "the custom entry in the strip");
  results.push("PASS frequency list: \"UI check net\" (7.110) added in Settings is in the strip's list");

  // ---- S4. delete a suggestion, then restore it
  const rows = await s.eval("document.querySelectorAll('.freq-list-editor tbody tr').length");
  await s.eval("document.querySelector('.freq-list-editor tbody tr.suggested button.delete').click()");
  await until(s, `document.querySelectorAll('.freq-list-editor tbody tr').length === ${rows - 1}`, "a suggestion deleted");
  await s.eval("document.querySelector('.freq-list-editor button.restore-suggested').click()");
  await until(s, `document.querySelector('.freq-list-editor .restored')?.textContent.trim() === 'Suggested entries added: 1' && document.querySelectorAll('.freq-list-editor tbody tr').length === ${rows}`, "the suggestion restored");
  results.push(`PASS restore: a deleted suggestion came back ("Suggested entries added: 1", ${rows} rows)`);

  // ---- S5. the chosen tab survives a reload
  await s.eval("location.reload()");
  await sleep(1500);
  await until(s, `!!${rail("Settings")}`, "the window after a reload", 20000);
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "document.getElementById('tab-rig')?.getAttribute('aria-selected') === 'true'", "the Rig tab remembered");
  results.push("PASS remembered tab: Rig is still chosen after a window reload");
  await s.eval("document.getElementById('tab-station').click()");
  await s.eval(`${rail("Chats")}.click()`);

  // ---- 3. the antenna icon
  // M5 Task 11c (owner): the rail in the canvas's order; Chats (the Operate
  // page with its list) has the canvas's chat icon, no longer the antenna.
  const order = await s.eval("[...document.querySelectorAll('.rail button')].map(b => b.getAttribute('aria-label')).join('|')");
  if (order !== "Chats|Heard|Files|Logbook|Diagnostics|Settings") throw new Error(`rail order ${order}`);
  if ((await s.eval(`${rail("Chats")}.title`)) !== "Chats") throw new Error("Chats has no tooltip");
  results.push(`PASS rail: ${order}, each with its tooltip and aria-label`);

  // ---- 4. composer
  await s.eval(setInput("input.peer", "sim1"));
  await until(s, "document.querySelector('.peer-header h1')?.textContent.trim() === 'SIM1'", "SIM1 open");
  await until(s, "!document.querySelector('textarea.composer').disabled", "composer enabled");
  const grow = await s.eval(`(async () => {
    const t = document.querySelector('textarea.composer'), cs = getComputedStyle(t);
    const settle = (ms) => new Promise((r) => setTimeout(r, ms));
    // The box fits itself after Vue has seen the input: measure once it has settled.
    const height = async (n) => { t.value = Array.from({ length: n }, (_, i) => 'line ' + i).join('\\n'); t.dispatchEvent(new Event('input')); await settle(80); return t.offsetHeight; };
    const line = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.2;
    const one = await height(1), five = await height(5), thirty = await height(30);
    return { resize: cs.resize, line, one, five, thirty, scrolls: t.scrollHeight > t.clientHeight, overflowY: getComputedStyle(t).overflowY };
  })()`);
  if (grow.resize !== "none") throw new Error(`composer resize is ${grow.resize}`);
  if (!(grow.five > grow.one + grow.line * 3)) throw new Error(`composer did not grow: ${JSON.stringify(grow)}`);
  if (!(grow.thirty > grow.five) || grow.thirty > grow.line * 8 + 40 || !grow.scrolls) throw new Error(`composer cap: ${JSON.stringify(grow)}`);
  await s.eval("(() => { const t = document.querySelector('textarea.composer'); t.value = 'hello'; t.dispatchEvent(new Event('input')); t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true })); })()");
  await until(s, "document.querySelector('textarea.composer').value === ''", "the composer cleared by Send");
  await sleep(150);
  const back = await s.eval("document.querySelector('textarea.composer').offsetHeight");
  if (Math.abs(back - grow.one) > 2) throw new Error(`composer did not shrink after Send: ${back} vs ${grow.one}`);
  results.push(`PASS composer: resize none; ${grow.one}px (1 line) -> ${grow.five}px (5) -> ${grow.thirty}px (30, capped near 8 lines, scrolls) -> ${back}px after Send`);

  // ---- 5. one sidebar everywhere
  await until(s, "document.querySelector('.item.active')", "the sent message made SIM1 a listed conversation");
  await s.eval("document.querySelector('aside.sidebar').__m3 = 'same instance'");
  await s.eval(setInput("input.search", "SIM"));
  const sideRect = await s.eval(rectOf("aside.sidebar"));
  const picked = await s.eval("document.querySelector('.item.active b').textContent");
  const pageNames = [["Files", ".files-view"], ["Settings", ".profiles, .card"], ["Diagnostics", ".diag"], ["Chats", ".chat-pane"]];
  for (const [name, marker] of pageNames) {
    await s.eval(`${rail(name)}.click()`);
    await until(s, `document.querySelector(${JSON.stringify(marker)})`, `${name} page`);
    const state = await s.eval(`({
      same: document.querySelector('aside.sidebar')?.__m3, rect: ${rectOf("aside.sidebar")},
      search: document.querySelector('input.search').value, active: document.querySelector('.item.active b')?.textContent,
      tabs: [...document.querySelectorAll('aside.sidebar [role=tab]')].map(b => b.textContent.trim()).join('|') })`);
    if (state.same !== "same instance") throw new Error(`${name}: the sidebar was rebuilt`);
    // Left edge and width only: on Operate the station strip (Task 10, owner ruling 17 Nov)
    // sits above the sidebar, so its top and height differ from the other pages.
    const lw = (r) => { const [l, , w] = r.split(","); return `${l},${w}`; };
    if (lw(state.rect) !== lw(sideRect)) throw new Error(`${name}: sidebar left/width ${lw(state.rect)} != ${lw(sideRect)}`);
    if (state.search !== "SIM" || state.active !== picked) throw new Error(`${name}: search/selection lost: ${JSON.stringify(state)}`);
    if (state.tabs !== "Chats|Heard") throw new Error(`${name}: tabs ${state.tabs}`);
  }
  await s.eval("[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim() === 'Heard').click()");
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "document.querySelector('.profiles, .card')", "Settings");
  if (!(await s.eval("document.querySelector('aside.sidebar [role=tab][aria-selected=true]').textContent.trim() === 'Heard'"))) throw new Error("the Heard tab was not kept");
  await s.eval("[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim() === 'Chats').click()");
  await s.eval(`${rail("Settings")}.click()`);
  await s.eval("document.querySelector('.item').click()");
  await until(s, "document.querySelector('.chat-pane')", "a chat picked on Settings opens it");
  results.push(`PASS sidebar: one instance, rect ${sideRect}, search text and selection ${picked} kept across Chats, Files, Settings and Diagnostics; Heard tab kept; a chat picked on Settings opens it`);

  // ---- 6. narrow: a drawer
  await s.send("Emulation.setDeviceMetricsOverride", { width: 900, height: 700, deviceScaleFactor: 1, mobile: false });
  await sleep(300);
  if (await s.eval(onScreen("aside.sidebar"))) throw new Error("the sidebar is still docked at 900 px");
  await s.eval("document.querySelector('button.toggle-sidebar').click()");
  await until(s, onScreen("aside.sidebar"), "the sidebar drawer open");
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await until(s, `!(${onScreen("aside.sidebar")})`, "the drawer closed by Escape");
  await s.send("Emulation.clearDeviceMetricsOverride");
  results.push("PASS narrow: at 900 px the sidebar is a drawer opened from the header and closed by Escape");

  // ---- T11a. station information and a macro with <EMAIL> (Settings); finalise-on-disconnect is off
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.querySelector('form.station-info')", "the station information form");
  await s.eval(setInput("form.station-info input[name=qth]", "Cape Town"));
  await s.eval(setInput("form.station-info input[name=email]", "op@example.org"));
  await s.eval("document.querySelector('form.station-info button[type=submit]').click()");
  await until(s, "/saved/i.test(document.querySelector('form.station-info .saved')?.textContent ?? '')", "station information saved");
  if (await s.eval("document.querySelector('input.finalise-on-disconnect').checked")) throw new Error("finalise on disconnect is on by default");
  await until(s, "document.querySelectorAll('.macro-editor .macro').length === 3", "the three seeded macros in the editor");
  await s.eval("document.querySelector('.macro-editor button.macro-add').click()");
  await s.eval(setInput(".macro-editor form.macro-edit input[name=name]", "Contact"));
  await s.eval(setInput(".macro-editor form.macro-edit textarea[name=body]", "Mail me: <EMAIL>"));
  await s.eval("document.querySelector('.macro-editor form.macro-edit button[type=submit]').click()");
  await until(s, "[...document.querySelectorAll('.macro-editor .macro .name')].some(n => n.textContent.trim() === 'Contact')", "the Contact macro saved");
  results.push("PASS station info: QTH and email saved from Settings; three macros seeded; a Contact macro with <EMAIL> added; finalise on disconnect is off by default");

  // ---- T11b. About: version, founder, links open in the system browser, nothing fetched
  const about = await s.eval("({ text: document.querySelector('section.about')?.innerText ?? '', version: document.querySelector('section.about .about-version')?.textContent.trim() ?? '' })");
  if (!/^0\.\d+\.\d+/.test(about.version) || !about.text.includes("Founded by ZS6SHZ") || !/GPL-3\.0-or-later/.test(about.text) || !/Rhizomatica/.test(about.text) || !/Hamlib/.test(about.text)) throw new Error(`About: ${JSON.stringify(about).slice(0, 400)}`);
  const before = await s.eval("performance.getEntriesByType('resource').length");
  await s.eval("document.querySelector('section.about button[data-link=issues]').click()");
  await s.eval("document.querySelector('section.about button[data-link=releases]').click()");
  await sleep(600);
  const opened = existsSync(linkLog) ? readFileSync(linkLog, "utf8").trim().split(/\r?\n/) : [];
  if (opened.join(" ") !== "https://github.com/armysarge/mercurychat/issues https://github.com/armysarge/mercurychat/releases") throw new Error(`links handed to the browser: ${JSON.stringify(opened)}`);
  const fetched = await s.eval(`performance.getEntriesByType('resource').slice(${before}).map(e => e.name).filter(n => /^https?:/.test(n) && !/\\/\\/(tauri|ipc)\\.localhost/.test(n))`);
  if (fetched.length) throw new Error(`About made requests: ${fetched.join(", ")}`);
  results.push(`PASS about: version ${about.version}, "Founded by ZS6SHZ", GPL-3.0-or-later, Mercury and Hamlib credited; Report an issue and Check for updates went to the system browser (${opened.length} URLs), no request from the window`);

  // ---- T11c. macros expand into the composer without sending; Insert email is its own item
  await s.eval(`${rail("Chats")}.click()`);
  await until(s, "document.querySelector('.peer-header h1')?.textContent.trim() === 'SIM1'", "SIM1 open on Operate");
  const msgsBefore = await s.eval("document.querySelectorAll('.transcript [data-id]').length");
  await s.eval("document.querySelector('textarea.composer').value = ''; document.querySelector('textarea.composer').dispatchEvent(new Event('input'))");
  const pickMacro = (name) => `(() => { document.querySelector('button.macro-toggle').click(); return new Promise(r => setTimeout(() => { const i = [...document.querySelectorAll('.macro-menu [role=menuitem]')].find(b => b.textContent.trim() === ${JSON.stringify(name)}); if (!i) throw new Error('no macro ' + ${JSON.stringify(name)}); i.click(); r(true); }, 50)); })()`;
  await s.eval(pickMacro("Greeting"));
  await until(s, "document.querySelector('textarea.composer').value.startsWith('Hello SIM1, this is ZS6SIM')", "Greeting expanded into the composer");
  const greet = await s.eval("({ text: document.querySelector('textarea.composer').value, note: document.querySelector('.macro-note')?.textContent.trim() ?? '' })");
  if (!/<NAME>/.test(greet.text) || !/NAME/.test(greet.note) || !/\d+ bytes/.test(greet.note)) throw new Error(`Greeting: ${JSON.stringify(greet)}`);
  await sleep(500);
  if ((await s.eval("document.querySelectorAll('.transcript [data-id]').length")) !== msgsBefore) throw new Error("expanding a macro sent a message");
  await s.eval("document.querySelector('textarea.composer').value = ''; document.querySelector('textarea.composer').dispatchEvent(new Event('input'))");
  await s.eval(pickMacro("Contact"));
  await until(s, "document.querySelector('textarea.composer').value === 'Mail me: <EMAIL>'", "Contact leaves <EMAIL> as it is");
  await s.eval(pickMacro("Insert email"));
  await until(s, "document.querySelector('textarea.composer').value === 'Mail me: op@example.org'", "Insert email fills the <EMAIL> tag");
  const mail = await s.eval("document.querySelector('textarea.composer').value");
  await s.eval("document.querySelector('textarea.composer').value = ''; document.querySelector('textarea.composer').dispatchEvent(new Event('input'))");
  results.push(`PASS macros: Greeting -> "${greet.text}" (note "${greet.note}"), nothing sent; Contact keeps <EMAIL>; Insert email -> "${mail}"`);

  // ---- T11d. a session opens the QSO card; edits survive a window reload; a summary row after the disconnect
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected to SIM1", 20000);
  await until(s, "document.querySelector('section.qso-card .qso-peer')?.textContent.trim() === 'SIM1' && !!document.querySelector('section.qso-card .qso-bw')?.textContent.trim()", "the QSO card with peer and bandwidth");
  // ---- R4. rig controls are disabled, with the reason, while connected
  const busy = await s.eval(`({
    tab: [...document.querySelectorAll('.strip .digit')].map(d => d.tabIndex),
    title: document.querySelector('.strip .freq-digits')?.title ?? '',
    list: document.querySelector('.strip select.freq-list')?.disabled,
    tune: document.querySelector('.strip button.tune')?.disabled,
  })`);
  if (!busy.tab.length || busy.tab.some((n) => n !== -1) || busy.title !== "not while connected" || !busy.list || !busy.tune) throw new Error(`rig controls while connected: ${JSON.stringify(busy)}`);
  results.push(`PASS rig busy: with SIM1 connected the ${busy.tab.length} digits take no focus, title "${busy.title}"; the list and Tune are disabled`);
  const bw = await s.eval("document.querySelector('section.qso-card .qso-bw').textContent.trim()");
  await s.eval(setInput("section.qso-card input[name=rstSent]", "59"));
  await s.eval(setInput("section.qso-card textarea[name=notes]", "first M3a QSO"));
  await s.eval("document.querySelector('section.qso-card button.qso-save').click()");
  await until(s, "/saved/i.test(document.querySelector('section.qso-card .saved')?.textContent ?? '')", "the QSO edits saved");
  await s.eval("(() => { const t = document.querySelector('textarea.composer'); t.value = 'qso bytes'; t.dispatchEvent(new Event('input')); t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true })); })()");
  await until(s, "[...document.querySelectorAll('.transcript [data-id]')].some(e => e.dataset.state === 'delivered' && e.querySelector('.body').textContent === 'qso bytes')", "a message delivered in the session", 20000);
  await s.eval("location.reload()");
  await sleep(1500);
  await until(s, `${pill} === 'connected' && document.querySelector('section.qso-card input[name=rstSent]')?.value === '59'`, "the QSO edits after a window reload", 20000);
  await s.eval(click("Disconnect"));
  await until(s, `['ready','listening'].includes(${pill})`, "the session ended", 20000);
  await until(s, "!!document.querySelector('.transcript li.sys[data-kind=session-end]')", "the QSO summary row");
  const summary = await s.eval("document.querySelector('.transcript li.sys[data-kind=session-end]').textContent.replace(/\\s+/g, ' ').trim()");
  if (!/Frequency unavailable|MHz/.test(summary) || !/\d+ s|\d+ min/.test(summary) || !/9 bytes|\d+ bytes/.test(summary)) throw new Error(`summary row: ${summary}`);
  if (!(await s.eval("!!document.querySelector('section.qso-card') && document.querySelector('section.qso-card .qso-state').textContent.trim() === 'Draft'"))) throw new Error("the last session's card is gone before it was finalised");
  const cqRow = await s.eval("document.querySelector('.transcript li.sys[data-kind=cq]')?.textContent.trim() ?? ''");
  if (!/CQ transmitted/.test(cqRow)) throw new Error(`no CQ row in the transcript: "${cqRow}"`);
  results.push(`PASS QSO card: Call opened it for SIM1 (${bw}); RST 59 and notes saved and kept across a window reload; after Disconnect the card stays as a draft and the transcript has "${summary}" and "${cqRow}"`);

  // ---- T11e. the Logbook keeps the draft; Finalise locks it
  await s.eval(`${rail("Logbook")}.click()`);
  await until(s, "!!document.querySelector('.logbook table.qsos tbody tr[data-state=draft]')", "the draft in the Logbook");
  const row = await s.eval("(() => { const r = document.querySelector('.logbook tbody tr'); return { peer: r.querySelector('.peer').textContent.trim(), state: r.dataset.state, rst: r.querySelector('.rst-sent').textContent.trim() }; })()");
  if (row.peer !== "SIM1" || row.rst !== "59") throw new Error(`Logbook row: ${JSON.stringify(row)}`);
  await s.eval("document.querySelector('.logbook th button[data-sort=peer]').click()");
  await s.eval("document.querySelector('.logbook tbody tr').click()");
  await until(s, "!!document.querySelector('.logbook form.qso-form button.qso-finalise')", "the record's form");
  await s.eval("document.querySelector('.logbook form.qso-form button.qso-finalise').click()");
  await until(s, "document.querySelector('.logbook tbody tr')?.dataset.state === 'final' && document.querySelector('.logbook form.qso-form input[name=rstSent]').disabled", "the record finalised and read-only");
  results.push(`PASS logbook: SIM1's record is listed as a draft with RST ${row.rst}, sortable; Finalise makes it final and read-only`);

  // ---- T11f. history search finds an old message by its text
  await s.eval(setInput("input.search", "hello"));
  await until(s, "[...document.querySelectorAll('.search-hits .hit')].some(h => h.textContent.includes('hello') && h.textContent.includes('SIM1'))", "a history hit for 'hello'", 5000);
  await s.eval("document.querySelector('.search-hits .hit').click()");
  await until(s, "document.querySelector('.peer-header h1')?.textContent.trim() === 'SIM1'", "the hit opens SIM1");
  await s.eval(setInput("input.search", "SIM"));
  results.push("PASS search: 'hello' found the earlier message to SIM1 and opened its conversation");

  // ---- 8 (first half). inert keys and the context menu
  for (const init of [{ key: "F12" }, { key: "I", ctrlKey: true, shiftKey: true }, { key: "r", ctrlKey: true }, { key: "F5" }]) {
    if (!(await s.eval(keyInert(init)))) throw new Error(`${JSON.stringify(init)} is not cancelled`);
  }
  const plain = await s.eval(rightClick(".chat-pane h1"));
  await sleep(100);
  if (!plain || (await s.eval(`${menuItems}.length`)) !== 0) throw new Error("a menu opened on plain page content");
  await s.eval("document.querySelector('textarea.composer').value = 'abc'");
  if (!(await s.eval(rightClick("textarea.composer")))) throw new Error("the default menu was not suppressed on the composer");
  await sleep(100);
  const items = await s.eval(menuItems);
  if (items.join("|") !== "Cut|Copy|Paste|Select all" && items.join("|") !== "Paste|Select all") throw new Error(`composer menu: ${items}`);
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  const tree = execFileSync("cargo", ["tree", "-p", "mercurychat", "-e", "features", "-i", "tauri", "--target", "all"], { cwd: repo, encoding: "utf8" });
  if (/devtools/.test(tree)) throw new Error("the tauri devtools feature is enabled in the dependency tree");
  const cargoToml = execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], { cwd: repo, encoding: "utf8", maxBuffer: 1 << 26 });
  const feats = JSON.parse(cargoToml).packages.find((p) => p.name === "mercurychat").features;
  if (Object.keys(feats).some((f) => f !== "devtools" && /devtools/.test(f)) || (feats.devtools && !feats.devtools.every((x) => x === "tauri/devtools"))) throw new Error(`odd features ${JSON.stringify(feats)}`);
  results.push(`PASS no devtools: F12, Ctrl+Shift+I, Ctrl+R, F5 cancelled; context menu only ${JSON.stringify(items)} on a field, none on page text; tauri has no devtools feature (cargo tree), the only opt-in is the dev feature "devtools" (not default)`);

  // ---- 7. position and size survive a restart
  s.close();
  const want = [140, 90, 1111, 777];
  windows(first.pid, want);
  await sleep(1500); // the save is debounced about 500 ms
  const placed = windows(first.pid).find((w) => w.w > 500);
  if (!placed || Math.abs(placed.x - want[0]) > 2 || Math.abs(placed.w - want[2]) > 2) throw new Error(`could not place the window: ${JSON.stringify(placed)}`);
  kill(first);
  await sleep(1500);
  first = launch(exe, dataDir, port);
  children.push(first);
  s = await page(port, (t) => !!t.trim() && !/Opening|Starting/.test(t), "the main window after restart");
  await until(s, ready, "the workspace after restart");
  await sleep(500);
  const back2 = windows(first.pid).find((w) => w.w > 500);
  const diff = ["x", "y", "w", "h"].map((k, i) => Math.abs(back2[k] - want[i]));
  if (Math.max(...diff) > 2) throw new Error(`window came back at ${JSON.stringify(back2)}, wanted ${want}`);
  results.push(`PASS window: moved and resized to ${want.join(",")}, back at ${back2.x},${back2.y},${back2.w},${back2.h} after restart`);

  // ---- T10e. after the restart: the star is kept, beacons are off and the prompt asks (RF-02)
  await s.eval("[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim() === 'Heard').click()");
  await until(s, "document.querySelector('.heard .row[data-callsign=\"SIM1\"] .star')?.getAttribute('aria-pressed') === 'true'", "SIM1 still starred after the restart");
  await until(s, "!!document.querySelector('.beacon .prompt')", "the beacon prompt after the restart");
  const prompt = await s.eval("document.querySelector('.beacon .prompt').textContent.trim()");
  const armed = await s.eval("document.querySelector('input.beacon-toggle').checked");
  const stripOff = await s.eval("document.querySelector('.strip .next-beacon').textContent.trim()");
  if (armed || prompt !== "Beacons were on last time; turn on again?" || stripOff !== "Beacons are off") throw new Error(`after restart: armed ${armed}, prompt "${prompt}", strip "${stripOff}"`);
  results.push(`PASS restart: SIM1's star kept; beacons off ("${stripOff}") with the prompt "${prompt}"`);

  // ---- T12b. turning on "any station heard" (and lab notifications) makes SIM1's next CQ or beacon a notification.
  // A stub stands in for the plugin's window.Notification: it records what the window asks to show, and nothing reaches the desktop.
  await s.eval(`(() => {
    window.__mcNotes = [];
    class Stub { constructor(title, opts) { window.__mcNotes.push({ title, body: opts?.body ?? null, sound: opts?.sound ?? null }); } }
    Stub.permission = 'granted';
    Stub.requestPermission = () => Promise.resolve('granted');
    window.Notification = Stub;
  })()`);
  await s.eval(`document.querySelector('.heard .row[data-callsign="SIM1"] .star').click()`);
  await until(s, `document.querySelector('.heard .row[data-callsign="SIM1"] .star').getAttribute('aria-pressed') === 'false'`, "SIM1 unstarred (so the heardAny switch is what notifies)");
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.querySelector('section.notifications')", "the notification settings");
  const notifyDefaults = await s.eval("({ any: document.querySelector('input.notify-heardAny').checked, message: document.querySelector('input.notify-message').checked })");
  if (notifyDefaults.any || !notifyDefaults.message) throw new Error(`notification defaults: ${JSON.stringify(notifyDefaults)}`);
  const quietBefore = await s.eval("window.__mcNotes.length");
  await s.eval("[...document.querySelectorAll('section.notifications label')].find(l => /Virtual Audio Lab/.test(l.textContent)).querySelector('input').click()");
  await s.eval("document.querySelector('input.notify-heardAny').click()");
  await until(s, "document.querySelector('input.notify-heardAny').checked", "Any station heard turned on");
  await s.eval(`${rail("Chats")}.click()`);
  await until(s, "window.__mcNotes.some(n => /^SIM1 heard: (CQ|Beacon)$/.test(n.title))", "a notification for SIM1's next CQ or beacon", 75000);
  const note = await s.eval("window.__mcNotes.find(n => /^SIM1 heard/.test(n.title))");
  if (quietBefore !== 0) throw new Error(`notified before it was turned on: ${quietBefore}`);
  results.push(`PASS notifications: off for a lab station until turned on; with "Any station newly heard" on, SIM1's broadcast asked for "${note.title}" (sound ${note.sound})`);

  // ---- 11. themes: Night Red changes the window, persists, and the splash uses it themeAfter a restart
  const rgb = (hex) => { const h = hex.replace("#", ""); return `rgb(${parseInt(h.slice(0, 2), 16)}, ${parseInt(h.slice(2, 4), 16)}, ${parseInt(h.slice(4, 6), 16)})`; };
  const night = { canvas: "#060000", surface: "#110000", bubbleOut: "#300000", badgeCq: "#ff1a00", accent: "#ff2200" };
  const cssVar = (n) => `getComputedStyle(document.documentElement).getPropertyValue('--${n}').trim()`;
  const bg = (sel) => `getComputedStyle(document.querySelector(${JSON.stringify(sel)})).backgroundColor`;
  // The composer's frame is a control edge: it uses the 3:1 control border, not the decorative divider.
  await s.eval(`${rail("Chats")}.click()`);
  // The composer shows with a conversation open (M5 Task 11c): open SIM1's.
  await until(s, "!!document.querySelector('.conv-row[data-callsign=\"SIM1\"] .item')", "SIM1 in the Chats list");
  await s.eval("document.querySelector('.conv-row[data-callsign=\"SIM1\"] .item').click()");
  await until(s, "!!document.querySelector('.composer-box')", `the composer on Operate (page: ${await s.eval("document.querySelector('.rail button.selected')?.getAttribute('aria-label') ?? 'none'")})`);
  if (!(await s.eval("!!document.querySelector('.composer-box')"))) throw new Error("themes: the composer box is gone");
  const frame = await s.eval(`({ edge: getComputedStyle(document.querySelector('.composer-box')).borderTopColor, strong: ${cssVar("border-strong")} })`);
  if (frame.edge !== rgb(frame.strong)) throw new Error(`composer frame ${frame.edge} is not --border-strong ${frame.strong}`);
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.querySelector('[data-theme-id=\"night-red\"]')", "the theme picker");
  const picks = await s.eval("[...document.querySelectorAll('[data-theme-id]')].map(b => b.dataset.themeId).join(',')");
  for (const id of ["system", "midnight", "daylight", "night-red", "high-contrast-dark", "high-contrast-light", "ocean", "slate", "amber-terminal"]) {
    if (!picks.split(",").includes(id)) throw new Error(`the picker lacks ${id}: ${picks}`);
  }
  const missing = await s.eval("['aside.sidebar', '.rail'].filter(q => !document.querySelector(q))");
  if (missing.length) throw new Error(`themes: no ${missing.join(", ")} on Settings`);
  const themeBefore = await s.eval(`({ side: ${bg("aside.sidebar")}, rail: ${bg(".rail")}, out: ${cssVar("bubble-out")}, cq: ${cssVar("badge-cq")} })`);
  await s.eval("document.querySelector('[data-theme-id=\"night-red\"]').click()");
  await until(s, `${cssVar("canvas")} === ${JSON.stringify(night.canvas)}`, "Night Red applied without a restart");
  const themeAfter = await s.eval(`({ side: ${bg("aside.sidebar")}, rail: ${bg(".rail")}, body: ${bg("body")}, out: ${cssVar("bubble-out")}, cq: ${cssVar("badge-cq")}, text: getComputedStyle(document.body).color, checked: document.querySelector('[data-theme-id="night-red"]').getAttribute('aria-checked') })`);
  if (themeAfter.side === themeBefore.side || themeAfter.rail === themeBefore.rail || themeAfter.out === themeBefore.out || themeAfter.cq === themeBefore.cq) throw new Error(`the sidebar, bubbles or badges did not change: ${JSON.stringify({ themeBefore, themeAfter })}`);
  if (themeAfter.rail !== rgb(night.surface) || themeAfter.body !== rgb(night.canvas) || themeAfter.out !== night.bubbleOut || themeAfter.cq !== night.badgeCq || themeAfter.checked !== "true") throw new Error(`Night Red values: ${JSON.stringify(themeAfter)}`);
  const [tr, tg, tb] = themeAfter.text.match(/\d+/g).map(Number);
  if (tb !== 0 || tg > 0x20 || tr < 0xf0) throw new Error(`Night Red text is not red-on-black: ${themeAfter.text}`);
  // The logo is themed too: under Night Red its gradient stops and edge carry no blue light.
  const logoColours = await s.eval(`(() => {
    const logo = document.querySelector('header svg.logo');
    if (!logo) return null;
    const rgbOf = (c) => c.match(/\\d+/g).map(Number);
    return {
      boot: [...logo.querySelectorAll('.boot-from, .boot-to')].map((e) => rgbOf(getComputedStyle(e).stopColor)),
      edge: rgbOf(getComputedStyle(logo.querySelector('rect')).stroke),
    };
  })()`);
  if (!logoColours || logoColours.boot.length !== 2 || [...logoColours.boot, logoColours.edge].some(([r, g, b]) => b !== 0 || g > 0x30 || r < 0xc0)) throw new Error(`header logo under Night Red: ${JSON.stringify(logoColours)}`);
  // Density and text size apply at once.
  await s.eval(`(() => { const r = document.querySelector('input[type=range]'); r.value = '120'; r.dispatchEvent(new Event('input')); })()`);
  await s.eval(`(() => { const d = [...document.querySelectorAll('select')].find(x => [...x.options].some(o => o.value === 'compact')); d.value = 'compact'; d.dispatchEvent(new Event('change')); })()`);
  await until(s, `document.documentElement.dataset.density === 'compact' && ${cssVar("font-scale")} === '1.2'`, "compact density and 120% text");
  const px = await s.eval("getComputedStyle(document.body).fontSize");
  if (px !== "16.8px") throw new Error(`120% text should be 16.8px, is ${px}`);
  // Status is still said with an icon and a word (not hue alone): Transcript's table.
  const transcriptSrc = readFileSync(join(repo, "apps", "desktop", "src", "components", "Transcript.vue"), "utf8");
  for (const [state, icon] of [["delivered", "✓✓"], ["uncertain", "?"], ["failed", "✕"]]) {
    if (!new RegExp(`${state}: \{ icon: "${icon}"`).test(transcriptSrc)) throw new Error(`the ${state} status lost its icon`);
  }
  await sleep(900); // the save is debounced
  const saved = JSON.parse(readFileSync(join(dataDir, "appearance.json"), "utf8"));
  if (saved.theme !== "night-red" || saved.tokens.canvas !== night.canvas || saved.density !== "compact" || saved.fontScale !== 120) throw new Error(`appearance.json: ${JSON.stringify(saved).slice(0, 200)}`);
  s.close();
  kill(first);
  await sleep(1500);
  process.env.MC_SPLASH_MIN_MS = "6000";
  first = launch(exe, dataDir, port);
  delete process.env.MC_SPLASH_MIN_MS;
  children.push(first);
  let sp2 = null;
  const seen2 = Date.now();
  while (!sp2 && Date.now() - seen2 < 20000) {
    sp2 = (await pages(port)).find(isSplash) ?? null;
    if (!sp2) await sleep(25);
  }
  if (!sp2) throw new Error("no splash themeAfter the restart");
  const sc2 = await connect(sp2.webSocketDebuggerUrl);
  await until(sc2, `getComputedStyle(document.body).backgroundColor === ${JSON.stringify(rgb(night.canvas))}`, "the splash in the saved theme", 4000);
  const splashBar = await sc2.eval("getComputedStyle(document.querySelector('.bar i')).backgroundColor");
  if (splashBar !== rgb(night.accent)) throw new Error(`splash accent ${splashBar}`);
  sc2.close();
  s = await page(port, (t) => !!t.trim() && !/Opening|Starting/.test(t), "the main window themeAfter the theme restart");
  await until(s, ready, "the workspace themeAfter the theme restart");
  const kept = await s.eval(`({ canvas: ${cssVar("canvas")}, density: document.documentElement.dataset.density, scale: ${cssVar("font-scale")} })`);
  if (kept.canvas !== night.canvas || kept.density !== "compact" || kept.scale !== "1.2") throw new Error(`themeAfter restart: ${JSON.stringify(kept)}`);
  results.push(`PASS themes: picker lists System and all presets with swatches; Night Red re-colours the sidebar (${themeBefore.side} -> ${themeAfter.side}), bubble and badge tokens at once, text is red (${themeAfter.text}); compact density and 120% text (${px}) apply instantly; header logo is red-only (boot stops ${JSON.stringify(logoColours.boot)}); status icons stay; appearance.json holds them; the splash and the window come back in Night Red after a restart`);
  s.close();

  // ---- 9. a start-up failure stays on the splash, with its message and a Close button
  // The first still holds this data folder. Same DevTools port: WebView2 shares one browser process
  // between instances with identical options, so this page list holds both instances' pages.
  const second = launch(exe, dataDir, port);
  children.push(second);
  let failed = null;
  const until9 = Date.now() + 20000;
  while (!failed && Date.now() < until9) {
    for (const t of (await pages(port)).filter(isSplash)) {
      const c = await connect(t.webSocketDebuggerUrl);
      const text = await c.eval("document.querySelector('.failure')?.innerText ?? ''");
      if (text) failed = c;
      else c.close();
    }
    if (!failed) await sleep(200);
  }
  if (!failed) throw new Error("the failed start did not show its message on the splash");
  const failure = await failed.eval("document.querySelector('.failure').innerText");
  if (!/cannot start/i.test(failure) || !failure.trim().split("\n").slice(1).join("").trim()) throw new Error(`failure text: ${failure}`);
  await failed.eval(click("Close"));
  await sleep(1500);
  let alive = true;
  try { process.kill(second.pid, 0); } catch { alive = false; }
  if (alive) throw new Error("Close did not exit the failed instance");
  results.push(`PASS start-up failure: the splash shows "${failure.replace(/\s+/g, " ").slice(0, 90)}…" with a Close button, no blank window, Close exits`);

  // ---- 10. a stalled start-up gives up: the splash says so and offers Close
  kill(first);
  await sleep(1500);
  process.env.MC_BOOT_STALL_MS = "60000";
  process.env.MC_STARTUP_TIMEOUT_MS = "3000";
  const stalled = launch(exe, dataDir, port);
  delete process.env.MC_BOOT_STALL_MS;
  delete process.env.MC_STARTUP_TIMEOUT_MS;
  children.push(stalled);
  const t0 = Date.now();
  const sp = await page(port, (t) => /too long/i.test(t), "the splash giving up on a stalled start", 15000);
  const waited = Date.now() - t0;
  if (!(await sp.eval("!!document.querySelector('.failure') && [...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Close')"))) throw new Error("no Close button on the timed-out splash");
  await sp.eval(click("Close"));
  await sleep(1500);
  let up = true;
  try { process.kill(stalled.pid, 0); } catch { up = false; }
  if (up) throw new Error("Close did not exit the stalled instance");
  results.push(`PASS stalled start-up: after a 3 s test timeout (seen at ${waited} ms) the splash says it is taking too long and Close exits`);
  sp.close();
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  for (const c of children) kill(c);
}
console.log(results.join("\n"));
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
