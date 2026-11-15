// M2 end-to-end check of the real desktop window's chat workspace, driven
// over the WebView2 DevTools protocol (simulator only, no RF):
//   1. first use: callsign + "Use simulator"; header says Receive-only
//   2. listen on/off: the header mode follows, and says so in words
//   3. call SIM1 -> "Connected to SIM1"; MercuryChat shown for the peer
//   4. Unicode message with line breaks, Ctrl+Enter -> "Delivered to remote
//      app" in text, and SIM1's echo arrives verbatim
//   5. 30 more messages -> the transcript pages; "Load older messages" works
//   6. 1200 px: the session pane is a drawer; Escape closes it, the session stays
//   7. 800x480: peer, mode, composer, Disconnect and Abort visible and on top
//   8. a burst of long messages, then Abort -> "uncertain" with Retry, nothing in limbo
//   9. restart -> history is there, not listening, nothing resent
// Usage: node lab/windows/m2-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
//   Against a real Mercury instead (Task 10, see m2-existing.mjs):
//   ... --existing HOST:PORT --peer SIM2 --peer-log FILE --stop-a CMD --start-a CMD
import { kill, launch, page, sleep, until } from "./cdp.mjs";
import { existingRun } from "./m2-existing.mjs";
import { button, click, compose, mode, pill, setInput, transcript } from "./m2-dom.mjs";

const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args.splice(i, 2)[1] : undefined;
};
const existing = flag("--existing");
const peerCall = flag("--peer") ?? "SIM2";
const peerLog = flag("--peer-log");
const stopA = flag("--stop-a");
const startA = flag("--start-a");
const [exe, dataDir, port = "9334"] = args;
if (!exe || !dataDir || (existing && !(peerLog && stopA && startA))) {
  console.error("usage: node m2-ui-check.mjs <mercurychat.exe> <data dir> [port] [--existing HOST:PORT --peer CALL --peer-log FILE --stop-a CMD --start-a CMD]");
  process.exit(2);
}
if (existing) {
  const [host, cp] = existing.split(":");
  const results = await existingRun({ exe, dataDir, port, host, controlPort: Number(cp), peer: peerCall, peerLog, stopA, startA });
  console.log(results.join("\n"));
  process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
}

const CALL = "ZS6SIM";
// A textarea normalises CR LF to LF, so the window sends LF line breaks; the
// core's CR LF round trip is covered by crates/mc-core/tests/chat.rs.
const TEXT = "héllo 👋\r\nline two\n\tשלום";

/** True when `sel` is inside the viewport and is what a click there would hit. */
const onTop = (sel) => `(() => {
  const e = document.querySelector(${JSON.stringify(sel)});
  if (!e) return 'missing';
  const r = e.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return 'hidden';
  if (r.left < 0 || r.top < 0 || r.right > innerWidth + 1 || r.bottom > innerHeight + 1) return 'outside ' + JSON.stringify([r.left, r.top, r.right, r.bottom]);
  const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
  return hit && (hit === e || e.contains(hit)) ? true : 'covered by ' + (hit?.className || hit?.tagName);
})()`;

const results = [];
let first, second;
try {
  first = launch(exe, dataDir, port);
  let s = await page(port, (t) => t.includes("Use simulator"), "the first-use screen");
  await s.eval(setInput('input[name="callsign"]', CALL));
  await s.eval(click("Use simulator"));
  await until(s, `${pill} === 'ready' && document.querySelector('textarea.composer')`, "the workspace, ready");
  if (await s.eval("document.querySelector('input.listen').checked")) throw new Error("listening was on by itself");
  const idle = await s.eval(mode);
  if (idle !== "Receive-only") throw new Error(`header mode "${idle}", expected Receive-only`);
  if (!(await s.eval(`document.querySelector('header').innerText.includes(${JSON.stringify(CALL)})`))) throw new Error("the header does not name the station");
  results.push(`PASS first use: ${CALL} simulator ready; header "${idle}", listening off`);

  await s.eval("document.querySelector('input.listen').click()");
  await until(s, `${mode} === 'Listening' && document.querySelector('.session-pane').textContent.includes('Incoming calls answered automatically')`, "Listening");
  await s.eval("document.querySelector('input.listen').click()");
  await until(s, `${mode} === 'Receive-only'`, "back to Receive-only");
  results.push("PASS listen: header Listening with the auto-answer text, then Receive-only");

  await s.eval(setInput("input.peer", "sim1"));
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected' && ${mode} === 'Connected to SIM1'`, "Connected to SIM1", 20000);
  const peerHeader = await s.eval("document.querySelector('.peer-header').innerText");
  if (!peerHeader.includes("SIM1") || !peerHeader.includes("MercuryChat")) throw new Error(`peer header: ${peerHeader}`);
  results.push("PASS call: header \"Connected to SIM1\", peer header names MercuryChat");

  const sent = await s.eval(compose(TEXT));
  const delivered = await until(
    s,
    `(() => { const t = ${transcript}; const m = t.find(m => m.direction === 'out' && m.body === ${JSON.stringify(sent)});
       const e = t.find(m => m.direction === 'in' && m.body === ${JSON.stringify("echo: " + sent)});
       return m && m.state === 'delivered' && e ? m : null; })()`,
    "delivered and echoed",
    20000,
  );
  if (!delivered.status.includes("Delivered to remote app")) throw new Error(`status text "${delivered.status}"`);
  if ((await s.eval("document.querySelector('textarea.composer').value")) !== "") throw new Error("composer not cleared");
  results.push(`PASS chat: ${new TextEncoder().encode(sent).length}-byte Unicode message, status "${delivered.status}", echo verbatim`);

  for (let i = 0; i < 30; i++) await s.eval(compose(`page ${i}`));
  await until(s, `(() => { const t = ${transcript}; return t.filter(m => m.direction === 'in' && m.body.startsWith('echo: page ')).length === 30; })()`, "30 echoes", 60000);
  // Reopening the conversation from the list loads its latest page only.
  await s.eval(`(${button("Conversations")}).click()`);
  await until(s, "[...document.querySelectorAll('.list .item')].some(b => b.textContent.includes('SIM1'))", "the conversation list");
  await s.eval("[...document.querySelectorAll('.list .item')].find(b => b.textContent.includes('SIM1')).click()");
  await sleep(500);
  await s.eval(`(${button("Operate")}).click()`);
  await until(s, `${transcript}.length > 0`, "the transcript reloaded");
  const paged = await s.eval(`${transcript}.length`);
  await s.eval(click("Load older messages"));
  await until(s, `${transcript}.length > ${paged}`, "older messages", 5000);
  const all = await s.eval(`${transcript}.length`);
  results.push(`PASS paging: ${paged} shown after reopening, ${all} after "Load older messages"`);

  await s.send("Emulation.setDeviceMetricsOverride", { width: 1200, height: 800, deviceScaleFactor: 1, mobile: false });
  await sleep(300);
  if ((await s.eval(onTop(".session-pane"))) === true) throw new Error("the session pane is still docked at 1200 px");
  await s.eval(click("Session"));
  await until(s, `(${onTop(".session-pane")}) === true`, "the session drawer open");
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await until(s, `(${onTop(".session-pane")}) !== true`, "the drawer closed by Escape");
  if ((await s.eval(pill)) !== "connected") throw new Error("Escape changed the session");
  results.push("PASS 1200 px: session pane is a drawer; Escape closed it and the session stayed up");

  await s.send("Emulation.setDeviceMetricsOverride", { width: 800, height: 480, deviceScaleFactor: 1, mobile: false });
  await sleep(300);
  const compact = {};
  for (const [name, sel] of [["peer", ".peer-header"], ["mode", "header .mode"], ["composer", "textarea.composer"], ["Disconnect", "button.disconnect"], ["Abort", "button.abort"]]) {
    compact[name] = await s.eval(onTop(sel));
  }
  const bad = Object.entries(compact).filter(([, v]) => v !== true);
  if (bad.length) throw new Error(`800x480: ${bad.map(([k, v]) => `${k} ${v}`).join("; ")}`);
  await s.send("Emulation.clearDeviceMetricsOverride");
  results.push("PASS 800x480: peer, mode, composer, Disconnect and Abort visible and on top");

  const long = "x".repeat(4000);
  await s.eval(`(() => { for (let i = 0; i < 5; i++) ${compose(long)}; })()`);
  await s.eval(click("Abort (delivery uncertain)"));
  await until(s, `${pill} === 'ready'`, "ready after abort", 40000);
  await sleep(500);
  const after = await s.eval(transcript);
  const longOnes = after.filter((m) => m.direction === "out" && m.body === long);
  const limbo = longOnes.filter((m) => m.state === "submitting" || m.state === "awaiting_receipt");
  const uncertain = longOnes.filter((m) => m.state === "uncertain");
  if (limbo.length) throw new Error(`${limbo.length} message(s) left ${limbo[0].state} after abort`);
  if (!uncertain.length) throw new Error(`no message became uncertain: ${longOnes.map((m) => m.state).join(", ")}`);
  if (!uncertain.every((m) => m.status.toLowerCase().includes("uncertain"))) throw new Error("uncertain is not said in words");
  if (!(await s.eval("[...document.querySelectorAll('.transcript [data-state=uncertain] button')].some(b => b.textContent.trim() === 'Retry')"))) {
    throw new Error("an uncertain message has no Retry button");
  }
  results.push(`PASS abort: ${longOnes.map((m) => m.state).join(", ")}; uncertain ones say so and offer Retry`);
  const echoesBefore = after.filter((m) => m.direction === "in").length;
  s.close();
  kill(first);
  await sleep(1500);

  second = launch(exe, dataDir, port);
  s = await page(port, (t) => t.includes(CALL) && t.includes("Operate"), "the restarted window");
  await until(s, `${pill} === 'ready' && ${transcript}.length > 0`, "history after restart", 15000);
  await s.eval(`(${button("Load older messages")})?.click()`);
  await sleep(3000);
  const again = await s.eval(transcript);
  if ((await s.eval(mode)) !== "Receive-only") throw new Error(`header mode after restart: ${await s.eval(mode)}`);
  const changed = again.filter((m) => {
    const before = after.find((b) => b.id === m.id && b.direction === m.direction);
    return before && before.state !== m.state;
  });
  if (again.filter((m) => m.direction === "in").length > echoesBefore) throw new Error("new incoming messages after restart");
  if (changed.length) throw new Error(`messages changed after restart: ${JSON.stringify(changed)}`);
  results.push(`PASS restart: ${again.length} message(s) in history unchanged, Receive-only, nothing resent`);
  s.close();
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  for (const c of [first, second]) if (c) kill(c);
}
console.log(results.join("\n"));
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
