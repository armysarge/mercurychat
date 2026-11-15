// M2 Task 10: the window check against a real Mercury (no RF), with the
// headless peer (`m0-client peer --echo`) on the other Mercury. Called by
// m2-ui-check.mjs when it is given --existing.
//   1. Existing profile on host:port, attached, listening off
//   2. call the peer -> connected
//   3. Unicode message -> delivered; the peer's sha256 equals ours, and the
//      peer stored it before the window showed delivered; echo verbatim
//   4. a long message, Mercury A stopped mid-message -> Recovering, uncertain
//   5. Mercury A started again -> Ready, not listening, nothing resent
//   6. call again, Retry -> delivered, stored once at the peer
import { createHash } from "node:crypto";
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { kill, launch, page, sleep, until } from "./cdp.mjs";

const TEXT = "héllo 👋\r\nline two\n\tשלום";
const pill = "document.querySelector('.pill')?.textContent.trim()";
const button = (label) =>
  `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(label)})`;
const click = (label) => `(() => { const b = ${button(label)}; if (!b) throw new Error('no button ' + ${JSON.stringify(label)}); b.click(); })()`;
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
const appCall = (cmd) => `window.__TAURI_INTERNALS__.invoke('app_call', { cmd: ${JSON.stringify(cmd)} })`;

/** The peer's JSON lines that record a stored message. */
function peerReceived(log) {
  try {
    return readFileSync(log, "utf8")
      .split("\n")
      .filter((l) => l.includes('"received"'))
      .map((l) => JSON.parse(l));
  } catch {
    return [];
  }
}

const sha = (t) => createHash("sha256").update(t, "utf8").digest("hex");

async function call(s, peer, ms) {
  // Mercury B may still hold the old link until its own timeout; keep trying.
  const end = Date.now() + ms;
  while (Date.now() < end) {
    await s.eval(click("Call"));
    const r = await until(s, `(${pill} === 'connected' && 'up') || (document.body.innerText.includes('Call failed') && 'failed') || (${pill} === 'ready' && 'ready')`, "a call outcome", 150000).catch(() => "timeout");
    if (r === "up") return;
    await sleep(5000);
  }
  throw new Error(`no session with ${peer} within ${ms / 1000} s`);
}

// stopA/startA are shell commands from the calling lab script (m2-cross-os.sh),
// not outside input, so they run through a shell on purpose.
export async function existingRun({ exe, dataDir, port, host, controlPort, peer, peerLog, stopA, startA }) {
  const results = [];
  let app;
  try {
    app = launch(exe, dataDir, port);
    let s = await page(port, (t) => t.includes("Use simulator"), "the window");
    const saved = await s.eval(`${appCall({
      type: "saveProfile",
      profile: {
        id: "", callsign: "ZS6WIN", name: "M2 lab", grid: null, lab: true, listenPreferred: false,
        modem: { kind: "existing", host, controlPort, dataPort: controlPort + 1, broadcastPort: null, telemetryUrl: null },
      },
    })}.then(p => p.id)`);
    await s.eval(appCall({ type: "selectProfile", id: saved }));
    await until(s, `${pill} === 'ready' && document.querySelector('.chat')`, "Mercury A attached", 30000);
    if (await s.eval("document.querySelector('input.listen').checked")) throw new Error("listening was on by itself");
    results.push(`PASS attach: Mercury at ${host}:${controlPort}, ready, listening off`);

    await s.eval(`(() => { const i = document.querySelector('input.peer'); i.value = ${JSON.stringify(peer)}; i.dispatchEvent(new Event('input')); })()`);
    let t0 = Date.now();
    await call(s, peer, 300000);
    results.push(`PASS call: ${peer} answered over Mercury, connected after ${((Date.now() - t0) / 1000).toFixed(1)} s`);

    const sent = await s.eval(compose(TEXT));
    t0 = Date.now();
    const id = await until(s, `(() => { const m = ${transcript}.find(m => m.direction === 'out' && m.body === ${JSON.stringify(sent)}); return m && m.state === 'delivered' ? m.id : null; })()`, "delivered", 180000);
    const deliveredSeen = Date.now();
    const atPeer = peerReceived(peerLog).filter((l) => l.id === id);
    if (atPeer.length !== 1) throw new Error(`the peer logged ${atPeer.length} copies of ${id}`);
    if (atPeer[0].sha256 !== sha(sent) || atPeer[0].bytes !== Buffer.byteLength(sent)) {
      throw new Error(`peer sha256 ${atPeer[0].sha256} (${atPeer[0].bytes} B) != ours ${sha(sent)} (${Buffer.byteLength(sent)} B)`);
    }
    const storedAt = Date.parse(atPeer[0].utc);
    if (!(storedAt <= deliveredSeen + 500)) throw new Error(`the peer stored it at ${atPeer[0].utc}, after the window showed delivered`);
    await until(s, `${transcript}.some(m => m.direction === 'in' && m.body === ${JSON.stringify("echo: " + sent)})`, "the echo", 180000);
    results.push(
      `PASS chat: ${Buffer.byteLength(sent)} B in ${((deliveredSeen - t0) / 1000).toFixed(1)} s; peer sha256 ${sha(sent).slice(0, 16)}… equal; ` +
        `peer stored at ${atPeer[0].utc}, window showed delivered ${new Date(deliveredSeen).toISOString()}; echo verbatim`,
    );

    const long = "y".repeat(3000);
    await s.eval(compose(long));
    const longId = await until(s, `(() => { const m = ${transcript}.find(m => m.direction === 'out' && m.body === ${JSON.stringify(long)} && (m.state === 'submitting' || m.state === 'awaiting_receipt')); return m ? m.id : null; })()`, "the long message in flight", 30000);
    await sleep(1500);
    execSync(stopA, { stdio: "inherit" });
    await until(s, `${pill} === 'recovering'`, "Recovering", 30000);
    await until(s, `${transcript}.find(m => m.id === ${JSON.stringify(longId)})?.state === 'uncertain'`, "uncertain", 30000);
    const atStop = peerReceived(peerLog).length;
    results.push(`PASS stop: Mercury A stopped mid-message -> recovering, message ${longId.slice(0, 8)}… uncertain`);

    execSync(startA, { stdio: "inherit" });
    const detail = await until(s, `${pill} === 'ready' && (document.querySelector('.state .detail')?.textContent.trim() ?? 'none')`, "Ready after restart", 90000);
    await sleep(10000);
    if (await s.eval(`${pill} === 'listening'`)) throw new Error("listening after recovery");
    if (peerReceived(peerLog).length !== atStop) throw new Error("the peer received something while nothing should be sent");
    if ((await s.eval(`${transcript}.find(m => m.id === ${JSON.stringify(longId)})?.state`)) !== "uncertain") throw new Error("the uncertain message changed by itself");
    results.push(`PASS restart: Mercury A back -> ready (${detail}), not listening, nothing resent in 10 s`);

    t0 = Date.now();
    await call(s, peer, 420000);
    await s.eval(`[...document.querySelectorAll('.transcript [data-id=${JSON.stringify(longId)}] button')].find(b => b.textContent.trim() === 'Retry').click()`);
    await until(s, `${transcript}.find(m => m.id === ${JSON.stringify(longId)})?.state === 'delivered'`, "the retry delivered", 240000);
    await sleep(3000);
    const copies = peerReceived(peerLog).filter((l) => l.id === longId);
    if (copies.length !== 1) throw new Error(`the peer logged ${copies.length} copies of the retried message`);
    results.push(`PASS retry: re-called after ${((Date.now() - t0) / 1000).toFixed(0)} s, Retry delivered ${longId.slice(0, 8)}… and the peer holds it once`);
    s.close();
  } catch (e) {
    results.push(`FAIL ${e.message}`);
  } finally {
    if (app) kill(app);
  }
  return results;
}
