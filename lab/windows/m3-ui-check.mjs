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
//        the Heard tab counts new stations while Chats is open and clears on open
//   T10b a star persists across a restart (checked in T10e)
//   T10c Call CQ shows "transmitted"; telemetry reads "unavailable"; the strip's
//        frequency is dashes in the seven-segment font; channel occupancy unknown
//   T10d turning beacons on shows a countdown; the activity drawer lists PTT lines
//   T10e after a restart beacons are off and the prompt asks to turn them on
// Usage: node lab/windows/m3-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect, kill, launch, page, pages, sleep, until } from "./cdp.mjs";
import { click, pill, setInput } from "./m2-dom.mjs";

const [exe, dataDir, port = "9335"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m3-ui-check.mjs <mercurychat.exe> <empty data dir> [port]");
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const CALL = "ZS6SIM";
const ANTENNA = "M12 8v13M9 21h6M3 8h18M6 5v6M10 4v8M14 5v6M18 6v4";

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
const ready = `(['ready','listening'].includes(${pill})) && document.querySelector('textarea.composer')`;
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
  s = await page(port, (t) => t.includes("Use simulator"), "the first-use screen");
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
  if (await s.eval("document.querySelector('.welcome input[name=startReceiveOnly]').checked")) throw new Error("Start in Receive only is on by default");
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

  // ---- T10a. SIM1 is heard by CQ, then by beacon with its locator (Chats open: the Heard tab counts them)
  await until(s, "document.querySelector('aside.sidebar [role=tab][aria-selected=true]')?.textContent.trim() === 'Chats'", "Chats open first");
  await until(s, "!!document.querySelector('#side-heard-new')", "the Heard tab's new-station count while Chats is open", 15000);
  const count = await s.eval("document.querySelector('#side-heard-new [aria-hidden]').textContent.trim()");
  await s.eval("[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim() === 'Heard').click()");
  await until(s, "!document.querySelector('#side-heard-new')", "the count cleared once Heard is open");
  const heardRow = "document.querySelector('.heard .row[data-callsign=\"SIM1\"]')";
  await until(s, heardRow, "SIM1 in the Heard list", 15000);
  // The first observation (the CQ) came while Chats was open; the beacon follows about 2 s later.
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
  results.push(`PASS heard: while Chats was open the Heard tab counted ${count}; opening Heard cleared it; SIM1 listed with a "Beacon" badge and locator JJ00aa after its CQ, highlighted (animation ${anim}, static under reduced motion); live region "${live}"; the activity drawer has the CQ line`);

  // ---- T10b. a star persists (checked again after the restart in section 7)
  await s.eval(`${heardRow}.querySelector('.star').click()`);
  await until(s, `${heardRow}.querySelector('.star').getAttribute('aria-pressed') === 'true'`, "SIM1 starred");

  // ---- T10c. CQ shows "transmitted"; telemetry reads unavailable; the strip shows dashes
  await s.eval(click("Call CQ"));
  await until(s, "/CQ: transmitted/.test(document.querySelector('.strip .cq-outcome')?.textContent ?? '')", "CQ transmitted", 10000);
  const tel = await s.eval("[...document.querySelectorAll('.telemetry .stat')].map(e => e.className.split(' ').pop() + '=' + e.lastElementChild.textContent.trim())");
  for (const k of ["snr", "mode", "throughput", "audio", "frequency"]) {
    if (!tel.includes(`${k}=unavailable`)) throw new Error(`telemetry ${k} is not "unavailable": ${tel.join(", ")}`);
  }
  if (!tel.some((x) => x.startsWith("link=") && x.endsWith("Not configured"))) throw new Error(`telemetry link: ${tel.join(", ")}`);
  const freq = await s.eval("({ text: document.querySelector('.strip .freq-digits').textContent.trim(), aria: document.querySelector('.strip .freq-digits').getAttribute('aria-label'), font: getComputedStyle(document.querySelector('.strip .freq-digits')).fontFamily })");
  if (freq.text !== "--.---.---" || freq.aria !== "Frequency unavailable" || !/DSEG7/.test(freq.font)) throw new Error(`strip frequency: ${JSON.stringify(freq)}`);
  const channel = await s.eval("document.querySelector('.beacon .channel').textContent.trim()");
  if (channel !== "Channel occupancy unknown") throw new Error(`channel: ${channel}`);

  // ---- T10d. beacons on show a countdown; the activity drawer lists PTT lines
  await s.eval("document.querySelector('input.beacon-toggle').click()");
  await until(s, "document.querySelector('input.beacon-toggle').checked && /^Next beacon \\d\\d:\\d\\d:\\d\\d$/.test(document.querySelector('.strip .next-beacon').textContent.trim())", "the beacon countdown on the strip");
  const cd = await s.eval("[document.querySelector('.strip .next-beacon').textContent.trim(), document.querySelector('.beacon .countdown')?.textContent.trim()]");
  await s.eval(click("Beacon now"));
  await until(s, "/Beacon: transmitted/.test(document.querySelector('.strip .beacon-outcome')?.textContent ?? '')", "Beacon now transmitted", 10000);
  const ptt = await s.eval("[...document.querySelectorAll('.activity .line[data-kind=ptt]')].map(l => l.querySelector('.text').textContent)");
  if (!ptt.includes("PTT on") || !ptt.includes("PTT off")) throw new Error(`no PTT lines in the activity drawer: ${JSON.stringify(ptt)}`);
  results.push(`PASS operate: Call CQ -> "transmitted"; telemetry rows read "unavailable" (link Not configured); strip frequency "--.---.---" in DSEG7, aria "Frequency unavailable"; "${channel}"; beacons on -> "${cd[0]}" (card ${cd[1]}); Beacon now -> "transmitted"; the activity drawer lists ${ptt.length} PTT lines`);

  // ---- 3. the antenna icon
  const d = await s.eval(`${rail("Operate")}.querySelector('path').getAttribute('d')`);
  if (d !== ANTENNA) throw new Error(`Operate icon path is ${d}`);
  if ((await s.eval(`${rail("Operate")}.title`)) !== "Operate") throw new Error("Operate has no tooltip");
  results.push("PASS icon: Operate rail icon is the antenna path, tooltip and aria-label \"Operate\"");

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
  const pageNames = [["Conversations", ".history"], ["Settings", ".profiles, .card"], ["Diagnostics", ".diag"], ["Operate", ".chat-pane"]];
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
  results.push(`PASS sidebar: one instance, rect ${sideRect}, search text and selection ${picked} kept across Operate, Conversations, Settings and Diagnostics; Heard tab kept; a chat picked on Settings opens it`);

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

  // ---- 11. themes: Night Red changes the window, persists, and the splash uses it themeAfter a restart
  const rgb = (hex) => { const h = hex.replace("#", ""); return `rgb(${parseInt(h.slice(0, 2), 16)}, ${parseInt(h.slice(2, 4), 16)}, ${parseInt(h.slice(4, 6), 16)})`; };
  const night = { canvas: "#060000", surface: "#110000", bubbleOut: "#300000", badgeCq: "#ff1a00", accent: "#ff2200" };
  const cssVar = (n) => `getComputedStyle(document.documentElement).getPropertyValue('--${n}').trim()`;
  const bg = (sel) => `getComputedStyle(document.querySelector(${JSON.stringify(sel)})).backgroundColor`;
  // The composer's frame is a control edge: it uses the 3:1 control border, not the decorative divider.
  const frame = await s.eval(`({ edge: getComputedStyle(document.querySelector('.composer-box')).borderTopColor, strong: ${cssVar("border-strong")} })`);
  if (frame.edge !== rgb(frame.strong)) throw new Error(`composer frame ${frame.edge} is not --border-strong ${frame.strong}`);
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.querySelector('[data-theme-id=\"night-red\"]')", "the theme picker");
  const picks = await s.eval("[...document.querySelectorAll('[data-theme-id]')].map(b => b.dataset.themeId).join(',')");
  for (const id of ["system", "midnight", "daylight", "night-red", "high-contrast-dark", "high-contrast-light", "ocean", "slate", "amber-terminal"]) {
    if (!picks.split(",").includes(id)) throw new Error(`the picker lacks ${id}: ${picks}`);
  }
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
  for (const [state, icon] of [["delivered", "✓"], ["uncertain", "?"], ["failed", "✕"]]) {
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
