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
//   O7 (Task 11c, owner) no conversation open: no peer header ("Type a
//      callsign"), no composer; the station home alone
//   O8 the station menu on a Heard row: right-click and the row's "…" open
//      it (Open chat, Call, Suggest QSY, Star; focus on its first item),
//      Escape closes it and gives focus back; Call from it calls SIM1
//   O9 the session header: Hang up and Abort in view, the rest in its "…"
//      menu (Export conversation, Suggest QSY, Star, Close conversation)
//   O10 Close conversation (after hanging up) brings the station home back
//   O11 a restart with history opens the station home, not the last conversation
//   O13 the header: "Beacon in mm:ss" while periodic beacons are on (gone
//      when off); the status capsule tinted by the modem's state
//   O14 the Chats list (canvas): its title, search, All/Sessions/CQ/Beacons
//      chips; SIM1 under "Live now" during the session with the last message
//      as its preview and a 40 px avatar; Call CQ / Beacon now at the foot,
//      held during the session with the reason, usable after hanging up
//   O15 the session header (canvas): "Connected · ... · since HH:MM UTC",
//      "Hang up", and a "QSO record" toggle whose aria-pressed follows the panel
//   O16 Heard starts empty at every start; the station heard this run is
//      listed; the Listen before transmit switch is on by default and off after
//      turning it off (Settings -> Modem)
//   O12 the rail in the canvas's order; Heard on it counts new stations and
//      opens the Heard list (the count clears)
// Usage: node lab/windows/m5-operate-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { firstUse, kill, launch, page, sleep, until } from "./cdp.mjs";
import { click, compose, pill, setInput } from "./m2-dom.mjs";
import { shoot } from "./responsive.mjs";

const [exe, dataDir, outDir, port = "9366"] = process.argv.slice(2);
if (!exe || !dataDir || !outDir) {
  console.error("usage: node m5-operate-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]");
  process.exit(2);
}
const CALL = "ZS6OPS";
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;
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
  await until(s, `['ready','listening'].includes(${pill}) && !!document.querySelector('.station-home')`, "the workspace", 30000);

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
  const idle = await s.eval(`({ header: !!document.querySelector('.peer-header'), composer: !!document.querySelector('textarea.composer'), typeCallsign: document.querySelector('main.view').innerText.includes('Type a callsign') })`);
  if (idle.header || idle.composer || idle.typeCallsign) throw new Error(`O7: with no conversation open the page shows ${JSON.stringify(idle)}`);
  results.push("PASS O7 no conversation open: no peer header, no composer, the station home alone");
  results.push(`PASS O1 station home: dial, bands "${await s.eval(text(".station-home .bands"))}", spectrum, CQ and Beacon cards`);

  // ---- O2: the header's station pill and status pill.
  const station = await s.eval(text("header .station-pill"));
  const status = await s.eval(text("header .state"));
  if (!station.includes(CALL)) throw new Error(`O2: the station pill says "${station}"`);
  // Owner, 24 Nov: the modem's kind is not in the title bar; it is in the capsule's tooltip.
  if (/Simulator/i.test(status)) throw new Error(`O2: the title bar still names the modem: "${status}"`);
  if (!/Simulator/i.test(await s.eval("document.querySelector('header .state').title"))) throw new Error("O2: the capsule's tooltip does not say where the modem is");
  results.push(`PASS O2 header: station "${station}"; status "${status}"`);
  await fits("operate-idle");

  // ---- O13: "Beacon in" in the header while beacons are on; the tinted capsule.
  if ((await s.eval("document.querySelector('header .state')?.dataset.tone")) !== "ok") throw new Error("O13: the status capsule is not tinted ok while ready");
  if (await s.eval("!!document.querySelector('header .beacon-in')")) throw new Error("O13: a beacon countdown while beacons are off");
  await s.eval("document.querySelector('.station-home input.beacon-toggle').click()");
  await until(s, `/^Beacon in [0-9][0-9]:[0-9][0-9]$/.test(${text("header .beacon-in")})`, "the header's beacon countdown", 15000);
  const beaconIn = await s.eval(text("header .beacon-in"));
  await s.eval("document.querySelector('.station-home input.beacon-toggle').click()");
  await until(s, "!document.querySelector('header .beacon-in')", "the countdown gone with beacons off", 15000);
  results.push(`PASS O13 header: "${beaconIn}" while beacons are on, gone when off; status capsule tinted ok`);

  // ---- O12: the rail; Heard's count, then the Heard list from the rail.
  const order = await s.eval("[...document.querySelectorAll('.rail button')].map(b => b.dataset.rail).join(',')");
  if (order !== "chats,heard,files,logbook,diagnostics,settings") throw new Error(`O12: rail order ${order}`);
  await until(s, "!!document.querySelector('.rail [data-rail=heard] .rail-badge')", "the Heard count on the rail", 20000);
  const badge = await s.eval(text(".rail [data-rail=heard] .rail-badge"));
  await s.eval("document.querySelector('.rail [data-rail=heard]').click()");
  await until(s, "document.querySelector('#side-tab-heard')?.getAttribute('aria-selected') === 'true' && !document.querySelector('.rail [data-rail=heard] .rail-badge')", "the Heard list open, the count cleared");
  if ((await s.eval("document.querySelector('.rail [data-rail=heard]').getAttribute('aria-current')")) !== "page") throw new Error("O12: Heard is not marked current");
  results.push(`PASS O12 rail: ${order}; Heard counted "${badge}", opened the Heard list and cleared`);

  // ---- O3: a session with SIM1, a message each way.
  await until(s, sim1, "SIM1 in the Heard list", 20000);
  // ---- O8: the station menu on SIM1's row.
  const row = "document.querySelector('.heard .row[data-callsign=\"SIM1\"]')";
  const menu = "document.querySelector('.station-menu[role=menu]')";
  const items = `[...(${menu}?.querySelectorAll('[role=menuitem]') ?? [])].map(b => b.dataset.action)`;
  await s.eval(`${row}.querySelector('.pick').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 120, clientY: 200 }))`);
  await until(s, `!!${menu}`, "the station menu from a right-click");
  const byClick = await s.eval(items);
  for (const a of ["open", "call", "suggest-qsy", "star"]) if (!byClick.includes(a)) throw new Error(`O8: the menu has no ${a}: ${byClick}`);
  if ((await s.eval(`document.activeElement?.dataset.action`)) !== byClick[0]) throw new Error("O8: focus is not on the menu's first item");
  await s.eval("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await until(s, `!${menu}`, "Escape closing the menu");
  const more = `${row}.querySelector('button.more')`;
  if (!(await s.eval(`!!${more}?.getAttribute('aria-label')`))) throw new Error("O8: the row has no named \"…\" button");
  await s.eval(`${more}.click()`);
  await until(s, `!!${menu}`, "the station menu from \"…\"");
  await s.eval("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await until(s, `!${menu}`, "Escape closing the menu");
  if (!(await s.eval(`document.activeElement === ${more}`))) throw new Error("O8: focus did not go back to \"…\"");
  await s.eval(`${more}.click()`);
  await until(s, `!!${menu}`, "the station menu again");
  await s.eval(`${menu}.querySelector('[data-action=call]').click()`);
  await until(s, "!!document.querySelector('.peer-header .avatar')", "SIM1's conversation with its avatar");
  results.push(`PASS O8 station menu: ${byClick.join(", ")}; right-click and "…" open it, Escape closes it, Call from it calls SIM1`);
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

  // ---- O14 (1): the Chats list during the session.
  await s.eval("document.getElementById('side-tab-chats').click()");
  await until(s, "!!document.querySelector('.conv .live-now .conv-row[data-callsign=\"SIM1\"]')", "SIM1 under Live now");
  const chats = await s.eval(`({
    title: ${text(".conv h2")},
    search: !!document.querySelector('.conv .search-box svg') && !!document.querySelector('.conv input.search'),
    chips: [...document.querySelectorAll('.conv .chips button')].map(b => b.textContent.trim() + ':' + b.getAttribute('aria-pressed')).join(','),
    preview: ${text(".conv .live-now .conv-row[data-callsign=\"SIM1\"] .preview")},
    avatar: document.querySelector('.conv .live-now .conv-row[data-callsign=\"SIM1\"] .avatar')?.getBoundingClientRect().width,
    cq: document.querySelector('.conv .foot button.foot-cq')?.disabled,
    beacon: document.querySelector('.conv .foot button.foot-beacon')?.disabled,
    why: ${text(".conv .foot .why")},
  })`);
  if (chats.title !== "Chats") throw new Error(`O14: the list's title is "${chats.title}"`);
  if (!chats.search) throw new Error("O14: no search box with its icon");
  if (chats.chips !== "All:true,Sessions:false,CQ:false,Beacons:false") throw new Error(`O14: chips ${chats.chips}`);
  if (!chats.preview.includes("Good afternoon from the restyle check")) throw new Error(`O14: SIM1's preview is "${chats.preview}"`);
  if (Math.round(chats.avatar) !== 40) throw new Error(`O14: the avatar is ${chats.avatar} px`);
  if (chats.cq !== true || chats.beacon !== true || !chats.why.includes("SIM1")) throw new Error(`O14: the foot during a session: ${JSON.stringify(chats)}`);

  // ---- O9: Hang up and Abort in view; the rest in the header's "…".
  const head = await s.eval(`({ hang: !!document.querySelector('.peer-header button.disconnect:not([disabled])'), abort: !!document.querySelector('.peer-header button.abort'), more: !!document.querySelector('.peer-header button.more') })`);
  if (!head.hang || !head.abort || !head.more) throw new Error(`O9: the session header has ${JSON.stringify(head)}`);
  await s.eval("document.querySelector('.peer-header button.more').click()");
  await until(s, `!!${menu}`, "the header's menu");
  const headItems = await s.eval(items);
  for (const a of ["export", "suggest-qsy", "star", "close"]) if (!headItems.includes(a)) throw new Error(`O9: the header menu has no ${a}: ${headItems}`);
  await s.eval("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await until(s, `!${menu}`, "Escape closing the header menu");
  results.push(`PASS O9 session header: Hang up and Abort in view; "…" holds ${headItems.join(", ")}`);

  // ---- O15: the session header's words and the QSO record toggle.
  const sub = await s.eval(text(".peer-header .who p"));
  if (!/^Connected · .* · since [0-9][0-9]:[0-9][0-9] UTC$/.test(sub)) throw new Error(`O15: the status line is "${sub}"`);
  const hang = await s.eval(text(".peer-header button.disconnect"));
  if (hang !== "Hang up") throw new Error(`O15: the hang-up button says "${hang}"`);
  const qso = "document.querySelector('.peer-header button.toggle-session')";
  if ((await s.eval(`${qso}.textContent.trim()`)) !== "QSO record") throw new Error("O15: the panel toggle is not \"QSO record\"");
  if ((await s.eval(`${qso}.getAttribute('aria-pressed')`)) !== "false") throw new Error("O15: the toggle says pressed with the panel closed");

  // ---- O4: the session panel with the QSO record.
  await s.eval(`${qso}.click()`);
  await until(s, "!!document.querySelector('.session-pane.open .qso-card, .session-pane .qso-card')", "the QSO record");
  if ((await s.eval(`${qso}.getAttribute('aria-pressed')`)) !== "true") throw new Error("O15: the toggle does not say pressed with the panel open");
  results.push(`PASS O15 session header: "${sub}"; "Hang up"; "QSO record" toggle pressed with the panel`);
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

  // ---- O10: hang up, then Close conversation: the station home again.
  await s.eval("document.querySelector('.peer-header button.disconnect').click()");
  await until(s, `['ready','listening'].includes(${pill})`, "the link down", 25000);
  await s.eval("document.querySelector('.peer-header button.more').click()");
  await until(s, `!!${menu}`, "the header's menu");
  await s.eval(`${menu}.querySelector('[data-action=close]').click()`);
  await until(s, "!!document.querySelector('.station-home') && !document.querySelector('.peer-header')", "the station home after closing");
  results.push("PASS O10 Close conversation: the station home again, no peer header");

  // ---- O14 (2): after hanging up, the foot's buttons are usable and the reason gone.
  await until(s, "document.querySelector('.conv .foot button.foot-cq')?.disabled === false && !document.querySelector('.conv .foot .why')", "Call CQ usable after hanging up");
  await s.eval("[...document.querySelectorAll('.conv .chips button')].find(b => b.textContent.trim() === 'CQ').click()");
  await until(s, "document.querySelector('.conv .chips button[aria-pressed=true]')?.textContent.trim() === 'CQ'", "the CQ chip on");
  await s.eval("[...document.querySelectorAll('.conv .chips button')].find(b => b.textContent.trim() === 'All').click()");
  results.push(`PASS O14 Chats list: "${chats.title}", search, chips ${chats.chips}; SIM1 live with "${chats.preview.slice(0, 50)}…", 40 px avatar; foot held ("${chats.why}") then usable`);

  results.push(...fit);
  const errs = (await s.eval("window.__uiErrors ?? []")) ?? [];
  if (errs.length) throw new Error(`O5 console errors: ${errs.join(" | ")}`);
  results.push("PASS O5 console: no errors");

  // ---- O11: a restart with SIM1's history opens the station home.
  s.close();
  kill(app);
  await sleep(2000);
  app = launch(exe, dataDir, port);
  s = await page(port, (t) => t.includes(CALL), "the window after a restart", 40000);
  await until(s, `['ready','listening'].includes(${pill}) && !!document.querySelector('.station-home')`, "the station home after a restart", 30000);
  if (await s.eval("!!document.querySelector('.peer-header')")) throw new Error("O11: the restart opened a conversation");
  results.push("PASS O11 restart with history: the station home, no conversation opened");

  // ---- O16: Heard is empty after the restart until something is heard; Listen before transmit.
  await s.eval("document.querySelector('.rail [data-rail=heard]').click()");
  await until(s, "document.querySelector('#side-tab-heard')?.getAttribute('aria-selected') === 'true'", "the Heard list");
  const early = await s.eval("document.querySelectorAll('.heard .row[data-callsign]').length");
  if (early !== 0) throw new Error(`O16: Heard lists ${early} stations at start, before anything was heard`);
  await until(s, "!!document.querySelector('.heard .row[data-callsign=\"SIM1\"]')", "SIM1 heard again this run", 75000);
  await s.eval("document.querySelector('.rail [data-rail=settings]').click()");
  await until(s, "!!document.getElementById('tab-modem')", "Settings");
  await s.eval("document.getElementById('tab-modem').click()");
  await until(s, "!!document.querySelector('input.busy-detect')", "the Listen before transmit switch");
  if (!(await s.eval("document.querySelector('input.busy-detect').checked"))) throw new Error("O16: Listen before transmit is off by default");
  await s.eval("document.querySelector('input.busy-detect').click()");
  await until(s, "document.querySelector('input.busy-detect').checked === false", "the switch off");
  results.push("PASS O16 Heard empty at start, SIM1 listed once heard; Listen before transmit on by default, off when switched");
} catch (e) {
  results.push(...fit);
  results.push(`FAIL ${e.message}`);
} finally {
  s?.close();
  kill(app);
}
for (const r of results) console.log(r);
process.exit(results.some((r) => r.startsWith("FAIL")) || results.length < 16 ? 1 : 0);
