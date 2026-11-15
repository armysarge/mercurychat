// M2 Task 8 end-to-end check of the real desktop window's chat test panel,
// driven over the WebView2 DevTools protocol (simulator only, no RF):
//   1. "Use simulator", listening stays off
//   2. call SIM1 -> connected
//   3. type a Unicode message with line breaks, Ctrl+Enter -> delivered, and
//      SIM1's echo arrives verbatim
//   4. a burst of long messages, then Abort -> "uncertain", nothing in limbo
//   5. restart -> history is there, not listening, nothing resent
// Usage: node lab/windows/m2-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
import { kill, launch, page, sleep, until } from "./cdp.mjs";

const [exe, dataDir, port = "9334"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m2-ui-check.mjs <mercurychat.exe> <data dir> [port]");
  process.exit(2);
}
const CALL = "ZS6SIM";
// A textarea normalises CR LF to LF, so the window sends LF line breaks; the
// core's CR LF round trip is covered by crates/mc-core/tests/chat.rs.
const TEXT = "héllo 👋\r\nline two\n\tשלום";

const pill = "document.querySelector('.pill')?.textContent.trim()";
const button = (label) =>
  `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(label)})`;
const click = (label) => `(() => { const b = ${button(label)}; if (!b) throw new Error('no button ' + ${JSON.stringify(label)}); b.click(); })()`;
// Every transcript entry as {id, state, direction, body} (body is the text node, verbatim).
const transcript = `[...document.querySelectorAll('.transcript [data-id]')].map(e => ({
  id: e.dataset.id, state: e.dataset.state, direction: e.dataset.direction,
  body: e.querySelector('.body').textContent }))`;
const compose = (text) => `(() => {
  const t = document.querySelector('textarea.composer');
  t.value = ${JSON.stringify(text)};
  t.dispatchEvent(new Event('input'));
  t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
  return t.value;
})()`;

const results = [];
let first, second;
try {
  first = launch(exe, dataDir, port);
  let s = await page(port, (t) => t.includes("Use simulator"), "the first window");
  await s.eval(`(() => {
    const i = document.querySelector('input[maxlength="15"]');
    i.value = ${JSON.stringify(CALL)};
    i.dispatchEvent(new Event('input'));
    ${button("Use simulator")}.click();
  })()`);
  await until(s, `${pill} === 'ready' && document.querySelector('.chat')`, "ready with the chat panel");
  const listen = await s.eval("document.querySelector('input.listen').checked");
  if (listen) throw new Error("listening was on by itself");
  if (await s.eval("document.body.innerText.includes('Incoming calls answered automatically')")) {
    throw new Error("the window claims incoming calls are answered");
  }
  results.push("PASS select: simulator ready, listening off, no auto-answer text");

  await s.eval(`(() => {
    const i = document.querySelector('input.peer');
    i.value = 'SIM1';
    i.dispatchEvent(new Event('input'));
  })()`);
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected", 20000);
  results.push("PASS call: SIM1 answered, state connected");

  const sent = await s.eval(compose(TEXT));
  const delivered = await until(
    s,
    `(() => { const t = ${transcript}; const m = t.find(m => m.direction === 'out' && m.body === ${JSON.stringify(sent)});
       const e = t.find(m => m.direction === 'in' && m.body === ${JSON.stringify("echo: " + sent)});
       return m && m.state === 'delivered' && e ? m.id : null; })()`,
    "delivered and echoed",
    20000,
  );
  const bytes = new TextEncoder().encode(sent).length;
  results.push(`PASS chat: ${bytes}-byte Unicode message ${delivered.slice(0, 8)}… delivered, echo verbatim`);
  if ((await s.eval("document.querySelector('textarea.composer').value")) !== "") throw new Error("composer not cleared");

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
  if (!(await s.eval("[...document.querySelectorAll('.transcript [data-state=uncertain] button')].some(b => b.textContent.trim() === 'Retry')"))) {
    throw new Error("an uncertain message has no Retry button");
  }
  results.push(`PASS abort: ${longOnes.map((m) => m.state).join(", ")}; uncertain ones offer Retry`);
  const echoesBefore = after.filter((m) => m.direction === "in").length;
  s.close();
  kill(first);
  await sleep(1500);

  second = launch(exe, dataDir, port);
  s = await page(port, (t) => t.includes(CALL) && t.includes("Chat"), "the restarted window");
  await until(s, `${pill} === 'ready' && ${transcript}.length >= ${after.length}`, "history after restart", 15000);
  await sleep(3000);
  const again = await s.eval(transcript);
  if (await s.eval(`${pill} === 'listening'`)) throw new Error("listening after restart");
  const changed = again.filter((m) => {
    const before = after.find((b) => b.id === m.id);
    return !before || (before.state !== m.state && m.direction === "out");
  });
  if (again.filter((m) => m.direction === "in").length !== echoesBefore) throw new Error("new incoming messages after restart");
  if (changed.length) throw new Error(`messages changed after restart: ${JSON.stringify(changed)}`);
  if (!again.some((m) => m.body === sent && m.state === "delivered")) throw new Error("the Unicode message is not in the history");
  results.push(`PASS restart: ${again.length} message(s) in history unchanged, state ready (not listening), nothing resent`);
  s.close();
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  for (const c of [first, second]) if (c) kill(c);
}
console.log(results.join("\n"));
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
