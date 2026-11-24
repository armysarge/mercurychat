// M4 Task 11 check of the real desktop window, driven over the WebView2
// DevTools protocol (simulator only: no RF, no audio device). SIM1 accepts
// any file after about 2 s; with MC_SIM_PEER_OFFERS=1 it offers notes.txt
// 5 s into a session and then a 640-px photo (sim::serve_peer_with).
//   F6 Settings → Files refuses more than 256 MiB (262145 KiB) and keeps 64 KiB (no 1 MiB cap, owner 24 Nov)
//   F2 SIM1 offers notes.txt: Accept → Complete, "Show in folder" present
//      (not clicked: it would open the file manager)
//   F3 SIM1 offers its photo: Decline → Declined on the card
//   F1 a file dropped on the conversation: the send dialog shows its sizes
//      and airtime, Send → Complete on SIM1
//   F5 with a minor-0 SIM1 (MC_SIM_PEER_MINOR0=1) Send file is disabled,
//      titled "not supported by this station"
//   F4 on a slow simulator link (MC_SIM_RATE=500) Cancel mid-send → Cancelled
//   F7 no console errors from the first screen to the end
// A drop is the window's own drag-and-drop event (tauri://drag-drop), sent
// from the page: the system file picker cannot be driven here.
// Usage: node lab/windows/m4-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
// Build the app with its assets embedded first:
//   npm --prefix apps/desktop run build && cargo build -p mercurychat --features tauri/custom-protocol
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { firstUse, kill, launch, page, sleep, until } from "./cdp.mjs";
import { click, pill, setInput } from "./m2-dom.mjs";

const [exe, dataDir, port = "9362"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m4-ui-check.mjs <mercurychat.exe> <empty data dir> [port]");
  process.exit(2);
}
const CALL = "ZS6SIM";
const ready = `(['ready','listening'].includes(${pill})) && document.querySelector('textarea.composer, .station-home')`;
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;
const invoke = (cmd) => `window.__TAURI_INTERNALS__.invoke('app_call', { cmd: ${JSON.stringify(cmd)} })`;
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
/** The transcript's card for the newest transfer matching `dir` and `name`. */
const card = (dir, name) =>
  `[...document.querySelectorAll('.transcript .transfer.${dir}')].filter(c => c.querySelector('.name')?.textContent.trim() === ${JSON.stringify(name)}).at(-1)`;
const drop = (path) =>
  `window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: 'tauri://drag-drop', payload: { paths: [${JSON.stringify(path)}], position: { x: 300, y: 300 } } })`;

const results = [];
const children = [];
let s;
let app;

async function start(env = {}) {
  Object.assign(process.env, env);
  const child = launch(exe, dataDir, port);
  for (const k of Object.keys(env)) delete process.env[k];
  children.push(child);
  return child;
}

async function restart(env, what) {
  s.close();
  kill(app);
  await sleep(2000);
  app = await start(env);
  s = await page(port, (t) => t.includes(CALL), what, 40000);
  await s.eval(watchErrors);
  await until(s, ready, `the workspace (${what})`, 30000);
}

/** Operate, SIM1's conversation open and a session with it. */
async function connectSim1() {
  await s.eval(`${rail("Chats")}.click()`);
  await s.eval(heardTab);
  await until(s, sim1, "SIM1 in the Heard list", 20000);
  await s.eval(`${sim1}.click()`);
  await until(s, "!!document.querySelector('button.macro-toggle')", "SIM1's conversation open");
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected to SIM1", 25000);
  await until(s, "!!document.querySelector('button.send-file-open')", "the Send file button");
}

async function noErrors(when) {
  const errs = (await s.eval("window.__uiErrors ?? []")) ?? [];
  if (errs.length) throw new Error(`console errors (${when}): ${errs.join(" | ")}`);
}

/** Bytes zstd can't shrink. */
function noise(n) {
  const out = [];
  let h = createHash("sha256").update("m4-ui-check").digest();
  while (out.length * 32 < n) {
    out.push(h);
    h = createHash("sha256").update(h).digest();
  }
  return Buffer.concat(out).subarray(0, n);
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  const srcDir = `${dataDir}-src`;
  mkdirSync(srcDir, { recursive: true });
  const hello = join(srcDir, "hello.txt");
  writeFileSync(hello, "Hello SIM1, a short note sent as a file. 73\n".repeat(40));
  const big = join(srcDir, "big.bin");
  writeFileSync(big, noise(60000));

  app = await start({ MC_SIM_PEER_OFFERS: "1" });
  s = await firstUse(port, "Use simulator", 40000);
  await s.eval(watchErrors);
  await s.eval(setInput('form.simulator input[name="callsign"]', CALL));
  await s.eval(click("Use simulator"));
  await until(s, ready, "the workspace", 30000);

  // ---- F6. Settings → Files: over 256 MiB is refused, 64 KiB is kept
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.getElementById('tab-files')", "Settings");
  await s.eval("document.getElementById('tab-files').click()");
  await until(s, "!!document.querySelector('form.file-settings input[name=max-kib]')", "the file settings");
  await s.eval(setInput("form.file-settings input[name=max-kib]", "262145"));
  await s.eval("document.querySelector('form.file-settings button[type=submit]').click()");
  await until(s, "!!document.querySelector('form.file-settings .err')", "the refusal of 256 MiB and over");
  const refusal = await s.eval("document.querySelector('form.file-settings .err').textContent.trim()");
  const stored = await s.eval(`${invoke({ type: "fileSettings" })}`);
  if (stored.maxIncoming !== 65536) throw new Error(`F6: 262145 KiB was stored: ${JSON.stringify(stored)}`);
  await s.eval(setInput("form.file-settings input[name=max-kib]", "64"));
  await s.eval("document.querySelector('form.file-settings button[type=submit]').click()");
  await until(s, "!!document.querySelector('form.file-settings .saved')", "64 KiB saved");
  results.push(`PASS F6 settings: 262145 KiB refused ("${refusal}"), 64 KiB saved`);

  // ---- F2. SIM1 offers notes.txt: Accept → Complete, Show in folder present
  await connectSim1();
  await until(s, `!!${card("in", "notes.txt")}?.querySelector('button.accept')`, "SIM1's offer of notes.txt", 20000);
  const offer = await s.eval(`${card("in", "notes.txt")}.textContent.replace(/\\s+/g, ' ').trim()`);
  await s.eval(`${card("in", "notes.txt")}.querySelector('button.accept').click()`);
  await until(s, `${card("in", "notes.txt")}?.dataset.state === 'complete'`, "notes.txt complete", 20000);
  const shown = await s.eval(`!!${card("in", "notes.txt")}.querySelector('button.show')`);
  if (!shown) throw new Error("F2: no Show in folder on the received file");
  results.push(`PASS F2 accept: "${offer.slice(0, 120)}" -> complete, with Show in folder`);

  // ---- F3. SIM1 offers its photo: Decline → Declined
  await until(s, `!!${card("in", "photo.jpg")}?.querySelector('button.decline')`, "SIM1's offer of photo.jpg", 20000);
  await s.eval(`${card("in", "photo.jpg")}.querySelector('button.decline').click()`);
  await until(s, `${card("in", "photo.jpg")}?.dataset.state === 'rejected'`, "photo.jpg declined", 10000);
  const declined = await s.eval(`${card("in", "photo.jpg")}.querySelector('.state').textContent.trim()`);
  if (declined !== "Declined") throw new Error(`F3: the card says "${declined}"`);
  results.push(`PASS F3 decline: the photo's card says "${declined}"`);

  // ---- F1. a dropped file: the dialog, Send, Complete
  await s.eval(drop(hello));
  await until(s, "!!document.querySelector('form.send-file .airtime')", "the send dialog with its estimate", 15000);
  const facts = await s.eval(`({
    size: document.querySelector('form.send-file .size').textContent.trim(),
    wire: document.querySelector('form.send-file .wire').textContent.trim(),
    airtime: document.querySelector('form.send-file .airtime').textContent.trim(),
  })`);
  await s.eval("document.querySelector('form.send-file button.send').click()");
  await until(s, "!document.querySelector('form.send-file')", "the dialog closed on Send");
  await until(s, `${card("out", "hello.txt")}?.dataset.state === 'complete'`, "hello.txt complete on SIM1", 30000);
  results.push(`PASS F1 send: size ${facts.size}, on the air ${facts.wire}, airtime ${facts.airtime}; SIM1 took it -> complete`);
  await s.eval(click("Disconnect")).catch(() => {});
  await noErrors("F1-F3, F6");

  // ---- F5. a minor-0 SIM1: Send file is disabled, with the reason
  await restart({ MC_SIM_PEER_MINOR0: "1" }, "a minor-0 SIM1");
  await connectSim1();
  const off = await s.eval("(() => { const b = document.querySelector('button.send-file-open'); return [b.disabled, b.title, document.getElementById('send-file-why')?.textContent.trim()]; })()");
  if (!off[0] || off[1] !== "not supported by this station" || off[2] !== "not supported by this station") throw new Error(`F5: ${JSON.stringify(off)}`);
  results.push(`PASS F5 minor 0: Send file disabled, "${off[1]}"`);
  await s.eval(click("Disconnect")).catch(() => {});
  await noErrors("F5");

  // ---- F4. a slow link: Cancel mid-send
  await restart({ MC_SIM_RATE: "500" }, "a 500 B/s simulator link");
  await connectSim1();
  await s.eval(drop(big));
  await until(s, "!!document.querySelector('form.send-file .airtime')", "the send dialog (60 KB)", 15000);
  await s.eval("document.querySelector('form.send-file button.send').click()");
  await until(s, `${card("out", "big.bin")}?.dataset.state === 'sending' && !!${card("out", "big.bin")}.querySelector('progress')`, "big.bin sending", 30000);
  await until(s, `Number(${card("out", "big.bin")}.querySelector('progress').value) > 0`, "some of big.bin on its way", 30000);
  await s.eval(`${card("out", "big.bin")}.querySelector('button.cancel').click()`);
  await until(s, `${card("out", "big.bin")}?.dataset.state === 'cancelled'`, "big.bin cancelled", 30000);
  const at = await s.eval(`${card("out", "big.bin")}.querySelector('.state').textContent.trim()`);
  results.push(`PASS F4 cancel mid-send: the card says "${at}"`);
  await s.eval(click("Disconnect")).catch(() => {});
  await noErrors("F4");
  results.push("PASS F7 console: no errors from the first screen to the end");
} catch (e) {
  results.push(`FAIL ${e.message.slice(0, 2000)}`);
  try {
    const out = children.at(-1)?.out ?? "";
    if (out) results.push(`app output (last 1500 chars): ${out.slice(-1500)}`);
  } catch {}
} finally {
  for (const c of children) kill(c);
}
console.log(results.join("\n"));
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
