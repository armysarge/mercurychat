// M3a Task 13: discovery, the beacon gate and telemetry loss over two real
// Mercury modems (no RF). Two mercury.exe over the crossed VB-Audio cables
// (WASAPI, PTT none, loopback only, telemetry WebSocket on), and two
// MercuryChat instances in lab mode (MC_DATA_DIR each), driven over the
// WebView2 DevTools protocol. Everything is read from the app's own state
// (snapshot, heard list, activity ring, events, QSO drafts) by invoking its
// typed commands in the page; the Operate screens are not used.
//
// Every app <-> modem socket goes through a logging loopback relay this
// script runs, so the control transcripts (what the app sent, what Mercury
// answered, with times) are evidence; station B's telemetry goes through its
// own relay, which the script closes mid-session.
//
//   M=<mercury.exe> node lab/windows/m3-discovery.mjs <mercurychat.exe> <fresh run dir>
//
// Checks: every CQ heard as one row with source CQ and the MONITOR SNR,
// whatever the CQFRAME/MONITOR order (F1); no telemetry SNR before a decode
// (F4); beacons into a broadcast port another client holds are named and hold
// after two (F3); a beacon heard with its locator; the session gate (beacon
// toggle, beacon now and CQ during a session: nothing on the air); exactly
// one beacon after the session with a 5-minute interval; telemetry lost while
// chat keeps delivering; frequency unavailable without a rig; session ends
// reach the peer promptly; Receive only mid-session waits for the session to
// end (F2). Plus the M3a real-Mercury questions: LISTEN OFF after
// DISCONNECTED vs the teardown frame, CQFRAME vs MONITOR order,
// PENDING/CANCELPENDING around our CQ, a second client on the broadcast port,
// and the telemetry status JSON's real types.
// Results: <run dir>/report.md and report.json; modem logs in <run dir>/{a,b}.
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import net from "node:net";
import { join, resolve } from "node:path";
import { connect, kill, launch, pages, sleep } from "./cdp.mjs";

const [exeArg, outArg] = process.argv.slice(2);
if (!exeArg || !outArg) {
  console.error("usage: M=<mercury.exe> node m3-discovery.mjs <mercurychat.exe> <fresh run dir>");
  process.exit(2);
}
const EXE = resolve(exeArg);
const OUT = resolve(outArg);
if (existsSync(OUT)) {
  console.error(`${OUT} exists: use a fresh run dir (Mercury locks files in an old one)`);
  process.exit(2);
}
const M = resolve(process.env.M ?? "lab/bin-patched/windows-x64/mercury.exe");
// This PC's VB-Audio ids (`mercury.exe -x wasapi -z`); never a system default or the radio's interface.
const CA_IN = process.env.CA_IN ?? "{0.0.0.00000000}.{1f68aa44-9ff8-4523-8429-02de487b53e0}"; // CABLE-A Input
const CA_OUT = process.env.CA_OUT ?? "{0.0.1.00000000}.{a3544a95-51f9-4cdc-aee9-359bd5b81053}"; // CABLE-A Output
const CB_IN = process.env.CB_IN ?? "{0.0.0.00000000}.{964afa2f-cfaa-4fca-90ad-66241370063e}"; // CABLE-B Input
const CB_OUT = process.env.CB_OUT ?? "{0.0.1.00000000}.{22742773-d4c6-4f25-bdd3-686ec5e365cb}"; // CABLE-B Output
const BEACON_MIN = Number(process.env.BEACON_MIN ?? 5);

const ST = {
  a: { call: "ZS1AAA", grid: "JF96fc", out: CA_IN, in: CB_OUT, ctl: 8300, bc: 8100, ui: 10000, rctl: 8310, rbc: 8110, rws: null, cdp: 9351 },
  b: { call: "ZS2BBB", grid: "KG44aa", out: CB_IN, in: CA_OUT, ctl: 8400, bc: 8200, ui: 10001, rctl: 8410, rbc: 8210, rws: 10011, cdp: 9352 },
};
const T0 = Date.now();
const rel = (t) => (t == null ? null : +((t - T0) / 1000).toFixed(3));
const iso = (t) => new Date(t).toISOString();
const log = (...a) => console.log(`[${rel(Date.now()).toFixed(1).padStart(7)} s]`, ...a);

const children = []; // { kind, proc }
const relays = [];
const results = []; // { name, pass, detail }
const report = { started_utc: iso(T0), checks: results, questions: {}, timings: {}, manifest: {}, telemetry_json: {}, relays: {}, events: {}, activity: {}, notes: [] };
const check = (name, pass, detail) => {
  results.push({ name, pass, detail });
  log(`${pass ? "PASS" : "FAIL"} ${name}: ${detail}`);
};

// ---------------------------------------------------------------- relays

/** A loopback TCP relay that logs what passes: text lines (control), frame
 * sizes and heads (broadcast), or byte counts only (data, telemetry). */
function relay(name, listen, target, mode) {
  const r = { name, listen, target, mode, entries: [], bytes: { up: 0, down: 0 }, conns: 0, sockets: new Set(), server: null };
  const tail = { up: "", down: "" };
  const note = (dir, chunk) => {
    const t = Date.now();
    r.bytes[dir] += chunk.length;
    if (mode === "text") {
      tail[dir] += chunk.toString("latin1");
      const parts = tail[dir].split(/\r\n|\r|\n/);
      tail[dir] = parts.pop();
      for (const line of parts) if (line.length) r.entries.push({ t, dir: dir === "up" ? "app>modem" : "modem>app", line });
    } else if (mode === "frames") {
      r.entries.push({ t, dir: dir === "up" ? "app>modem" : "modem>app", n: chunk.length, head: chunk.subarray(0, 48).toString("hex") });
    }
  };
  r.server = net.createServer((client) => {
    r.conns++;
    const up = net.connect(target, "127.0.0.1");
    r.sockets.add(client);
    r.sockets.add(up);
    r.entries.push({ t: Date.now(), dir: "relay", line: `connection ${r.conns} opened` });
    client.on("data", (d) => {
      note("up", d);
      up.write(d);
    });
    up.on("data", (d) => {
      note("down", d);
      client.write(d);
    });
    const end = (who) => () => {
      if (!client.destroyed || !up.destroyed) r.entries.push({ t: Date.now(), dir: "relay", line: `connection ${r.conns} closed by ${who}` });
      client.destroy();
      up.destroy();
      r.sockets.delete(client);
      r.sockets.delete(up);
    };
    client.on("close", end("app"));
    up.on("close", end("modem"));
    client.on("error", () => {});
    up.on("error", () => {});
  });
  r.server.listen(listen, "127.0.0.1");
  r.close = () =>
    new Promise((done) => {
      r.entries.push({ t: Date.now(), dir: "relay", line: "relay closed by the lab script" });
      r.closedAt = Date.now();
      for (const s of r.sockets) s.destroy();
      r.server.close(() => done());
    });
  relays.push(r);
  return r;
}

/** Control lines in [from, to] (ms epoch), as "+s dir line". */
const lines = (r, from = 0, to = Infinity, re = null) =>
  r.entries.filter((e) => e.t >= from && e.t <= to && e.line && (!re || re.test(e.line)));
const firstLine = (r, re, from = 0, dir = null) => r.entries.find((e) => e.t >= from && e.line && re.test(e.line) && (!dir || e.dir === dir));
const show = (es) => es.map((e) => `${rel(e.t)} ${e.dir} ${e.line ?? `${e.n} B ${e.head}`}`);

// ---------------------------------------------------------------- processes

function cleanup() {
  for (const c of children) {
    try {
      if (c.kind === "app") kill(c.proc);
      else execFileSync("taskkill", ["/F", "/T", "/PID", String(c.proc.pid)], { stdio: "ignore" });
    } catch {}
  }
  for (const r of relays) {
    for (const s of r.sockets) s.destroy();
    try {
      r.server.close();
    } catch {}
  }
}
process.on("SIGINT", () => {
  cleanup();
  process.exit(130);
});

function startModem(id) {
  const s = ST[id];
  const dir = join(OUT, id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "mercury.ini"), "[main]\nbind_address = 127.0.0.1\n[store]\nenabled = false\n");
  const args = ["-C", join(dir, "mercury.ini"), "-x", "wasapi", "-o", s.out, "-i", s.in, "-p", String(s.ctl), "-b", String(s.bc), "-P", "none", "-G", "-U", String(s.ui), "-L", join(dir, "mercury.log")];
  const proc = spawn(M, args, { cwd: dir, stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  proc.stdout.on("data", (d) => (out += d));
  proc.stderr.on("data", (d) => (out += d));
  proc.on("exit", () => writeFileSync(join(dir, "stdout.log"), out));
  children.push({ kind: "modem", proc });
  report.manifest[`mercury_${id}_args`] = args.join(" ");
  return proc;
}

/** Connect-only readiness probe: never writes to a modem port. */
async function portOpen(port, ms = 15000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const ok = await new Promise((done) => {
      const c = net.connect(port, "127.0.0.1");
      c.on("connect", () => {
        c.destroy();
        done(true);
      });
      c.on("error", () => done(false));
    });
    if (ok) return true;
    await sleep(200);
  }
  throw new Error(`port ${port} did not open`);
}

/** Mercury's log lines as { t (ms epoch, local clock), text }. */
function modemLog(id) {
  try {
    return readFileSync(join(OUT, id, "mercury.log"), "utf8")
      .split(/\r?\n/)
      .map((l) => {
        const m = /^(\d{4}-\d\d-\d\d) (\d\d:\d\d:\d\d\.\d{3}) (.*)$/.exec(l);
        return m ? { t: new Date(`${m[1]}T${m[2]}`).getTime(), text: m[3] } : null;
      })
      .filter(Boolean);
  } catch {
    return [];
  }
}
const modemLines = (id, re, from = 0, to = Infinity) => modemLog(id).filter((l) => l.t >= from && l.t <= to && re.test(l.text));

// ---------------------------------------------------------------- telemetry JSON capture (Q6)

/** A plain WebSocket client on a modem's UI port: records how each status
 * field is written on the wire (integer, decimal, null, string...). */
function wsCapture(id, url) {
  const cap = { url, status: 0, binary: 0, other: {}, forms: {}, samples: [], opened: null, error: null };
  const FIELDS = ["radio_frequency_hz", "radio_frequency_age_ms", "freq_offset_hz", "freq_offset_age_ms", "snr", "listen_mode", "busy", "bitrate", "peer_snr", "arq_tx_mode", "audio_ok"];
  const form = (raw) => (raw === "null" ? "null" : /^-?\d+$/.test(raw) ? "integer" : /^-?\d+\.\d+$/.test(raw) ? "decimal" : raw.startsWith('"') ? "string" : /^(true|false)$/.test(raw) ? "bool" : `other:${raw.slice(0, 12)}`);
  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  ws.onopen = () => (cap.opened = Date.now());
  ws.onerror = (e) => (cap.error = String(e?.message ?? e?.type ?? e));
  ws.onmessage = (m) => {
    if (typeof m.data !== "string") {
      cap.binary++;
      return;
    }
    const type = /"type":"([a-z_]+)"/.exec(m.data)?.[1] ?? "?";
    if (type !== "status") {
      cap.other[type] = (cap.other[type] ?? 0) + 1;
      return;
    }
    cap.status++;
    for (const f of FIELDS) {
      const raw = new RegExp(`"${f}":("[^"]*"|[^,}]+)`).exec(m.data)?.[1];
      const k = raw === undefined ? "absent" : form(raw);
      cap.forms[f] ??= {};
      const e = (cap.forms[f][k] ??= { n: 0, example: raw });
      e.n++;
    }
    // Keep the first frame, and one with a decoded offset, as raw evidence.
    if (cap.samples.length === 0 || (cap.samples.length === 1 && /"freq_offset_hz":-?\d/.test(m.data))) cap.samples.push(m.data);
  };
  cap.close = () => ws.close();
  report.telemetry_json[id] = cap;
  return cap;
}

// ---------------------------------------------------------------- app instances

const COLLECTOR = `(async () => {
  if (window.__lab) return 'already';
  const T = window.__TAURI_INTERNALS__;
  const inv = T.invoke.bind(T);
  const snap = await inv('app_call', { cmd: { type: 'snapshot' } });
  window.__lab = { events: [], seq: snap.seq, err: null, ui: [], wrapped: false };
  window.__labInvoke = inv;
  // Commands the window itself sends (a click on its own buttons) are logged:
  // anything but this script acting on the modem is evidence to keep.
  try {
    const quiet = ['snapshot', 'heard', 'activity', 'conversations', 'history', 'qsos', 'macros', 'saveDraft'];
    T.invoke = (c, a, o) => {
      if (c === 'app_call' && a && a.cmd && !quiet.includes(a.cmd.type)) window.__lab.ui.push({ t: Date.now(), cmd: JSON.stringify(a.cmd).slice(0, 200) });
      return inv(c, a, o);
    };
    window.__lab.wrapped = T.invoke !== inv;
  } catch (e) { window.__lab.err = 'wrap: ' + e; }
  (async () => {
    for (;;) {
      try {
        const r = await inv('events_since', { seq: window.__lab.seq });
        if (r.type === 'events') {
          for (const e of r.events) { window.__lab.events.push({ t: Date.now(), ...e }); window.__lab.seq = e.seq; }
        } else {
          const s = await inv('app_call', { cmd: { type: 'snapshot' } });
          window.__lab.events.push({ t: Date.now(), resync: true, seq: s.seq });
          window.__lab.seq = s.seq;
        }
      } catch (e) { window.__lab.err = String(e); }
      await new Promise((r) => setTimeout(r, 120));
    }
  })();
  return 'started';
})()`;

const inst = {};

async function startApp(id) {
  const s = ST[id];
  const data = join(OUT, `app-${id}`);
  mkdirSync(data, { recursive: true });
  // Each instance its own WebView2 profile: two browsers with different
  // DevTools ports cannot share one user data folder.
  process.env.WEBVIEW2_USER_DATA_FOLDER = join(data, "webview2");
  process.env.MC_LOG = process.env.MC_LOG ?? "info";
  const proc = launch(EXE, data, s.cdp);
  delete process.env.WEBVIEW2_USER_DATA_FOLDER;
  children.push({ kind: "app", proc });
  const end = Date.now() + 60000;
  let sess = null;
  while (!sess && Date.now() < end) {
    for (const t of await pages(s.cdp)) {
      if (t.url?.includes("splash")) continue;
      try {
        const c = await connect(t.webSocketDebuggerUrl);
        if (await c.eval("!!window.__TAURI_INTERNALS__ && document.readyState === 'complete'")) {
          sess = c;
          break;
        }
        c.close();
      } catch {}
    }
    if (!sess) await sleep(300);
  }
  if (!sess) throw new Error(`app ${id}: no main page on DevTools port ${s.cdp} (${proc.out.slice(0, 400)})`);
  await sess.eval(COLLECTOR);
  const me = { id, s, sess, proc, events: [] };
  inst[id] = me;
  return me;
}

async function cmd(id, c) {
  const r = await inst[id].sess.eval(
    `window.__labInvoke('app_call', { cmd: ${JSON.stringify(c)} }).then((ok) => ({ ok }), (err) => ({ err }))`,
  );
  return r;
}
async function must(id, c) {
  const r = await cmd(id, c);
  if (r.err) throw new Error(`${id} ${c.type}: ${JSON.stringify(r.err)}`);
  return r.ok;
}
const snap = (id) => must(id, { type: "snapshot" });
const heard = async (id) => (await must(id, { type: "heard" })).stations;

async function pull() {
  for (const me of Object.values(inst)) {
    try {
      const evs = await me.sess.eval("window.__lab ? window.__lab.events.splice(0) : []");
      me.events.push(...evs);
    } catch {}
  }
}
const pump = setInterval(pull, 250);

/** Events of instance `id` of AppEvent type `type`, after `from`. */
const evs = (id, type, from = 0) => inst[id].events.filter((e) => e.t >= from && e.event?.type === type);

async function until(fn, ms, what, step = 250) {
  const end = Date.now() + ms;
  for (;;) {
    await pull();
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out after ${ms / 1000} s waiting for ${what}`);
    await sleep(step);
  }
}
const idle = (st) => st === "ready" || st === "listening";

// ---------------------------------------------------------------- session helpers

/** A calls `peer` (retrying while the modems settle); returns { t, up }. */
async function callUp(from, to, ms = 300000, during = null) {
  const end = Date.now() + ms;
  const peer = ST[to].call;
  while (Date.now() < end) {
    await until(async () => idle((await snap(from)).modem.type) && idle((await snap(to)).modem.type), 300000, "both modems idle before the call");
    const t = Date.now();
    await must(from, { type: "call", peer });
    if (during) await during();
    const r = await until(
      async () => {
        const a = await snap(from);
        if (a.modem.type === "connected" && a.session) return { ok: true, a };
        const failed = evs(from, "callFailed", t)[0];
        if (failed) return { ok: false, why: failed.event.reason };
        return null;
      },
      200000,
      `the call ${from} -> ${to}`,
    ).catch((e) => ({ ok: false, why: e.message }));
    if (r.ok) {
      const peerSession = await until(async () => (await snap(to)).session, 60000, `${to} to see the session`);
      // Each side keeps its own session id.
      return { t, up: Date.now(), sessionId: r.a.session.sessionId, peerSessionId: peerSession.sessionId };
    }
    report.notes.push(`call ${from}->${to} failed (${r.why}); retrying`);
    log(`call failed: ${r.why}; retrying in 10 s`);
    await sleep(10000);
  }
  throw new Error(`no session ${from} -> ${to}`);
}

/** Send a chat and wait until it is delivered at the sender and received at the peer. */
async function chat(from, to, text, ms = 240000) {
  const t = Date.now();
  await must(from, { type: "sendChat", peer: ST[to].call, text });
  await until(() => evs(from, "messageChanged", t).find((e) => e.event.message.direction === "out" && e.event.message.body === text && e.event.message.state === "delivered"), ms, `"${text}" delivered`);
  await until(() => evs(to, "messageChanged", t).find((e) => e.event.message.direction === "in" && e.event.message.body === text), 30000, `"${text}" at ${to}`);
  return (Date.now() - t) / 1000;
}

/** End the session from `who`; measure when each side's control port reported DISCONNECTED. */
async function endSession(who, other, label) {
  const R = { a: relays.find((r) => r.name === "a-ctl"), b: relays.find((r) => r.name === "b-ctl") };
  const t = Date.now();
  await must(who, { type: "disconnect" });
  const sent = await until(() => firstLine(R[who], /^DISCONNECT$/, t, "app>modem"), 150000, `${who} to send DISCONNECT`);
  const mine = await until(() => firstLine(R[who], /^DISCONNECTED$/, t, "modem>app"), 300000, `${who}'s DISCONNECTED`);
  const theirs = await until(() => firstLine(R[other], /^DISCONNECTED$/, t, "modem>app"), 330000, `${other}'s DISCONNECTED`).catch(() => null);
  await until(async () => idle((await snap(who)).modem.type) && idle((await snap(other)).modem.type), 330000, "both idle after the session").catch(() => null);
  await sleep(4000);
  const window = (r) => show(lines(r, sent.t - 50, Date.now()).filter((e) => !/^(SN|BITRATE|BUFFER) /.test(e.line)));
  const out = {
    label,
    ended_by: who,
    disconnect_sent_s: rel(sent.t),
    ender_disconnected_t: mine.t,
    ender_disconnected_after_s: +((mine.t - sent.t) / 1000).toFixed(3),
    peer_disconnected_after_s: theirs ? +((theirs.t - sent.t) / 1000).toFixed(3) : null,
    // What each app sent after its own DISCONNECTED, ms after it.
    ender_listen_after: lines(R[who], mine.t, Date.now(), /^LISTEN /).filter((e) => e.dir === "app>modem").map((e) => `${e.line} +${e.t - mine.t} ms`),
    peer_listen_after: theirs ? lines(R[other], theirs.t, Date.now(), /^LISTEN /).filter((e) => e.dir === "app>modem").map((e) => `${e.line} +${e.t - theirs.t} ms`) : [],
    ender_ptt_after_disconnected: lines(R[who], mine.t, Date.now(), /^PTT /).map((e) => `${e.line} +${e.t - mine.t} ms`),
    peer_ptt_after_disconnected: theirs ? lines(R[other], theirs.t, Date.now(), /^PTT /).map((e) => `${e.line} +${e.t - theirs.t} ms`) : [],
    ender_modem_log: modemLines(who, /LISTEN|isconnect|PTT/i, sent.t - 500, Date.now()).map((l) => `${rel(l.t)} ${l.text}`),
    peer_modem_log: modemLines(other, /LISTEN|isconnect|PTT/i, sent.t - 500, Date.now()).map((l) => `${rel(l.t)} ${l.text}`),
    transcript_ender: window(R[who]),
    transcript_peer: window(R[other]),
  };
  return out;
}

// ---------------------------------------------------------------- run

let A, B, wsA, wsB;
try {
  mkdirSync(OUT, { recursive: true });
  const sha = (f) => createHash("sha256").update(readFileSync(f)).digest("hex");
  report.manifest.mercury = M;
  report.manifest.mercury_sha256 = sha(M);
  try {
    report.manifest.mercury_manifest = readFileSync(join(M, "..", "MANIFEST.txt"), "utf8").trim();
  } catch {}
  report.manifest.app = EXE;
  report.manifest.app_sha256 = sha(EXE);
  report.manifest.audio = "wasapi; A out CABLE-A Input / in CABLE-B Output; B out CABLE-B Input / in CABLE-A Output; PTT none; bind_address 127.0.0.1";
  log(`mercury ${report.manifest.mercury_sha256.slice(0, 16)}…, app ${report.manifest.app_sha256.slice(0, 16)}…`);

  // ---- modems, relays, telemetry capture
  startModem("a");
  startModem("b");
  for (const p of [8300, 8301, 8100, 10000, 8400, 8401, 8200, 10001]) await portOpen(p, 30000);
  for (const id of ["a", "b"]) {
    const s = ST[id];
    relay(`${id}-ctl`, s.rctl, s.ctl, "text");
    relay(`${id}-data`, s.rctl + 1, s.ctl + 1, "bytes");
    relay(`${id}-bc`, s.rbc, s.bc, "frames");
  }
  const wsRelay = relay("b-ws", ST.b.rws, ST.b.ui, "bytes");
  wsA = wsCapture("a", `ws://127.0.0.1:${ST.a.ui}/websocket`);
  wsB = wsCapture("b", `ws://127.0.0.1:${ST.b.ui}/websocket`);
  log("modems up; relays listening");

  // F3: another program holds Mercury A's one broadcast-client slot before the
  // app attaches. The app's connection (through a-bc) then waits in the OS
  // backlog: it connects, its writes are taken, and nothing reaches the modem.
  const holder = { connected: null, closed: null, bytes: 0 };
  {
    const t = Date.now();
    const sock = net.connect(ST.a.bc, "127.0.0.1");
    sock.on("connect", () => (holder.connected = Date.now()));
    sock.on("data", (d) => (holder.bytes += d.length));
    sock.on("close", () => (holder.closed = Date.now()));
    sock.on("error", () => {});
    holder.sock = sock;
    await until(() => modemLines("a", /tcp-bcast] Client connected/, t - 50).length > 0 && holder.connected, 10000, "Mercury A to take the holder as its broadcast client");
    log("A's broadcast port is held by a raw client");
  }

  // ---- the two app instances, each on its own modem through the relays
  for (const id of ["a", "b"]) {
    const s = ST[id];
    await startApp(id);
    const telemetryUrl = `ws://127.0.0.1:${s.rws ?? s.ui}/websocket`;
    const p = await must(id, {
      type: "saveProfile",
      profile: { id: "", callsign: s.call, name: null, grid: s.grid, lab: true, receiveOnly: false, modem: { kind: "existing", host: "127.0.0.1", controlPort: s.rctl, dataPort: s.rctl + 1, broadcastPort: s.rbc, telemetryUrl } },
    });
    s.profileId = p.id;
    await must(id, { type: "selectProfile", id: p.id });
  }
  A = inst.a;
  B = inst.b;
  await until(async () => (await snap("a")).modem.type === "listening" && (await snap("b")).modem.type === "listening", 90000, "both apps attached and listening");
  await until(async () => (await snap("a")).telemetry.link.type === "up" && (await snap("b")).telemetry.link.type === "up", 30000, "telemetry up on both");
  report.timings.attached_s = rel(Date.now());
  log("both apps attached, listening, telemetry up");

  // ---- Q6 / frequency: telemetry JSON as written, and what the app makes of it
  await until(() => wsA.status >= 6 && wsB.status >= 6, 20000, "status frames on the raw captures");
  // F4: before anything is decoded Mercury writes snr 0.0 (no decode yet:
  // freq_offset_age_ms null); the app must show no SNR, not 0 dB.
  {
    const [sa, sb] = [await snap("a"), await snap("b")];
    const forms = (c) => ({ snr: Object.keys(c.forms.snr ?? {}).join("/"), snr_example: Object.values(c.forms.snr ?? {})[0]?.example, age: Object.keys(c.forms.freq_offset_age_ms ?? {}).join("/") });
    const fa = forms(wsA);
    const fb = forms(wsB);
    report.f4_before_decode = { wire_a: fa, wire_b: fb, app_a: sa.telemetry.snrDb, app_b: sb.telemetry.snrDb, at_s: rel(Date.now()) };
    report.f4_ok_idle = fa.age === "null" && fb.age === "null" && sa.telemetry.snrDb === null && sb.telemetry.snrDb === null;
    log(`F4 before any decode: wire A snr ${fa.snr} (${fa.snr_example}) age ${fa.age}; app A snrDb ${JSON.stringify(sa.telemetry.snrDb)}, B ${JSON.stringify(sb.telemetry.snrDb)}`);
  }
  {
    const sa = await snap("a");
    const f = wsA.forms;
    const freqNull = f.radio_frequency_hz && Object.keys(f.radio_frequency_hz).join() === "null";
    report.questions.q6_telemetry_ws = {
      url: `ws://127.0.0.1:<ui port>/websocket (mercury -G -U <port>)`,
      status_frames_a: wsA.status,
      binary_frames_a: wsA.binary,
      other_events_a: wsA.other,
      forms_a_idle: JSON.parse(JSON.stringify(f)),
      first_status_a: wsA.samples[0],
      app_view_a: sa.telemetry,
    };
    check(
      "frequency unavailable without a rig",
      freqNull && sa.telemetry.frequency === null && sa.telemetry.link.type === "up",
      `wire radio_frequency_hz ${JSON.stringify(f.radio_frequency_hz)}; app telemetry link ${sa.telemetry.link.type}, frequency ${JSON.stringify(sa.telemetry.frequency)} (shown as unavailable), listenMode ${sa.telemetry.listenMode}`,
    );
  }

  // ---- F3: beacons into a held broadcast port are named, and hold after two
  {
    const BCA0 = relays.find((r) => r.name === "a-bc");
    const t = Date.now();
    const f3 = { outcomes: [], refusals: [] };
    try {
      await must("a", { type: "setBeacon", on: true });
      for (let i = 0; i < 2; i++) {
        const ti = Date.now();
        const r = await cmd("a", { type: "sendBeaconNow" });
        if (r.err) {
          f3.refusals.push(JSON.stringify(r.err));
          continue;
        }
        const o = await until(() => evs("a", "broadcast", ti).find((e) => e.event.kind === "beacon" && e.event.outcome.type !== "submitted"), 40000, "the beacon outcome").catch(() => null);
        f3.outcomes.push(o ? { s: rel(o.t), after_s: +((o.t - ti) / 1000).toFixed(1), outcome: o.event.outcome } : { outcome: "none in 40 s" });
      }
      await sleep(1500);
      await pull();
      const bs = (await snap("a")).beacon;
      f3.status_after = bs;
      f3.app_wrote = BCA0.entries.filter((e) => e.t >= t && e.dir === "app>modem").map((e) => `${rel(e.t)} ${e.n} B`);
      f3.modem_a_took = modemLines("a", /KISS frame decoded|Broadcast frame queued/, t).map((l) => `${rel(l.t)} ${l.text}`);
      f3.b_heard = evs("b", "heard", t).filter((e) => e.event.station.source === "beacon").length;
      f3.activity = evs("a", "activity", t).filter((e) => e.event.entry.kind === "broadcast").map((e) => `${rel(e.t)} ${e.event.entry.text}`);
      const named = f3.outcomes.filter((o) => o.outcome?.type === "notConfirmed" && o.outcome.reason === "broadcastPortSilent").length;
      const why = "the modem did not transmit; another program may be using Mercury's broadcast port";
      const held = bs.active && bs.nextDueUtc === null && bs.suppressed === why && f3.activity.some((l) => l.includes("beacons held"));
      check(
        "F3: a held broadcast port is named, and beacons hold after two",
        named === 2 && held && f3.modem_a_took.length === 0 && f3.b_heard === 0,
        `2 Beacon now into A's held port: ${f3.outcomes.map((o) => `${o.outcome?.type}${o.outcome?.reason ? ` (${o.outcome.reason})` : ""} after ${o.after_s} s`).join(", ")}${f3.refusals.length ? `; refused ${f3.refusals.join(" ")}` : ""}; ` +
          `app wrote ${f3.app_wrote.join(", ")} (Mercury took ${f3.modem_a_took.length}, B heard ${f3.b_heard}); beacon status: active ${bs.active}, nextDue ${bs.nextDueUtc}, suppressed "${bs.suppressed}"; activity: ${f3.activity.join(" | ")}`,
      );
    } catch (e) {
      check("F3: a held broadcast port is named, and beacons hold after two", false, e.message);
    }
    await cmd("a", { type: "setBeacon", on: false });
    // Free the slot: Mercury then accepts the app's waiting connection, with
    // whatever the app wrote into it meanwhile.
    const tr = Date.now();
    holder.sock.destroy();
    await sleep(20000);
    await pull();
    f3.released_s = rel(tr);
    f3.modem_a_after_release = modemLines("a", /tcp-bcast|Broadcast frame queued|PTT/, tr).map((l) => `${rel(l.t)} ${l.text}`);
    f3.b_heard_after_release = evs("b", "heard", tr).filter((e) => e.event.station.source === "beacon").map((e) => `${rel(e.t)} ${e.event.station.callsign}`);
    f3.stale_sent = modemLines("a", /Broadcast frame queued/, tr).length;
    report.f3 = f3;
    log(`F3 port released: Mercury A queued ${f3.stale_sent} stale beacon frame(s); B heard ${f3.b_heard_after_release.length}`);
    // The app said those beacons were not transmitted; they must not go out later.
    check(
      "beacons reported not transmitted stay off the air when the port frees",
      f3.stale_sent === 0 && f3.b_heard_after_release.length === 0,
      `holder closed at +${f3.released_s} s; Mercury A then: ${f3.modem_a_after_release.join(" | ") || "nothing"}; B heard ${f3.b_heard_after_release.join(", ") || "nothing"}`,
    );
  }

  // ---- Q5: a second KISS client on B's broadcast port, beside the app (through b-bc)
  const extra = { connected: null, closed: null, error: null, chunks: [] };
  {
    const t = Date.now();
    const sock = net.connect(ST.b.bc, "127.0.0.1");
    sock.on("connect", () => (extra.connected = Date.now()));
    sock.on("data", (d) => extra.chunks.push({ t: Date.now(), n: d.length, head: d.subarray(0, 48).toString("hex") }));
    sock.on("close", () => (extra.closed = Date.now()));
    sock.on("error", (e) => (extra.error = e.message));
    extra.sock = sock;
    await sleep(3000);
    extra.modem_log = modemLines("b", /tcp-bcast/, t - 1000).map((l) => l.text);
    log(`second broadcast client: connected=${!!extra.connected} closed=${!!extra.closed} ${extra.error ?? ""}`);
  }

  // ---- CQ (Q3, Q4)
  const cqRuns = [];
  for (const [from, to, n] of [["a", "b", 3], ["b", "a", 2]]) {
    for (let i = 0; i < n; i++) {
      const Rf = relays.find((r) => r.name === `${from}-ctl`);
      const Rt = relays.find((r) => r.name === `${to}-ctl`);
      const before = (await heard(to)).find((x) => x.callsign === ST[from].call)?.count ?? 0;
      const t = Date.now();
      const r = await cmd(from, { type: "sendCq" });
      const run = { from, to, i, refused: r.err ?? null };
      if (!r.err) {
        const outcome = await until(() => evs(from, "broadcast", t).find((e) => e.event.kind === "cq" && e.event.outcome.type !== "submitted"), 40000, "the CQ outcome").catch(() => null);
        run.outcomes = evs(from, "broadcast", t).filter((e) => e.event.kind === "cq").map((e) => `${rel(e.t)} ${e.event.outcome.type}${e.event.outcome.reason ? ` (${e.event.outcome.reason})` : ""}`);
        run.final = outcome?.event.outcome.type ?? "none in 40 s";
        const h = await until(() => evs(to, "heard", t).find((e) => e.event.station.callsign === ST[from].call && e.event.station.source === "cq"), 40000, "the CQ heard").catch(() => null);
        await sleep(2500);
        await pull();
        const row = (await heard(to)).find((x) => x.callsign === ST[from].call);
        run.heard_after_s = h ? +((h.t - t) / 1000).toFixed(2) : null;
        run.row = row ? { source: row.source, snr: row.snr, bandwidth: row.bandwidth, freq: row.freq, offsetHz: row.offsetHz, count: row.count } : null;
        run.rows_added = (row?.count ?? 0) - before;
        run.heard_lines = evs(to, "activity", t).filter((e) => e.event.entry.kind === "heard").map((e) => e.event.entry.text);
        run.heard_events = evs(to, "heard", t).filter((e) => e.event.station.callsign === ST[from].call).length;
        run.sender = show(lines(Rf, t, Date.now()).filter((e) => /^(CQFRAME|OK|WRONG|PENDING|CANCELPENDING|PTT|BUSY)/.test(e.line)));
        run.receiver = show(lines(Rt, t, Date.now()).filter((e) => /^(CQFRAME|MONITOR)/.test(e.line)));
        const cq = firstLine(Rt, /^CQFRAME /, t);
        const mon = firstLine(Rt, /^MONITOR /, t);
        run.order = cq && mon ? (cq.t < mon.t || (cq.t === mon.t && Rt.entries.indexOf(cq) < Rt.entries.indexOf(mon)) ? "CQFRAME first" : "MONITOR first") : `CQFRAME ${!!cq}, MONITOR ${!!mon}`;
        run.order_gap_ms = cq && mon ? mon.t - cq.t : null;
      }
      cqRuns.push(run);
      log(`CQ ${from}->${to} #${i + 1}: ${run.final ?? "refused"}, heard ${run.heard_after_s ?? "-"} s, ${run.order}, snr ${JSON.stringify(run.row?.snr)}`);
      await sleep(6000);
    }
  }
  report.questions.q3_q4_cq = cqRuns;
  {
    // F1: every CQ, both directions and whatever the line order, is one row with the MONITOR SNR.
    const ok = cqRuns.every((r) => r.row?.source === "cq" && r.row?.snr?.source === "modem monitor" && Number.isFinite(r.row?.snr?.value) && r.rows_added === 1);
    check("CQ heard with source CQ and the MONITOR SNR (F1: one row, either line order)", ok, cqRuns.map((r) => `${r.from}->${r.to} #${r.i + 1}: ${r.row?.source} snr ${r.row?.snr ? `${r.row.snr.value} dB (${r.row.snr.source})` : "none"}, ${r.order}, rows +${r.rows_added}, after ${r.heard_after_s} s`).join("; "));
    // The task 13F brief also asks for one heard event per CQ, whatever the order.
    const once = cqRuns.every((r) => r.heard_lines.length === 1 && r.heard_events === 1);
    check("one heard activity line and one heard event per CQ, either order", once, cqRuns.map((r) => `${r.from}->${r.to} #${r.i + 1} ${r.order}: lines ${r.heard_lines.length} (${r.heard_lines.join(" / ")}), heard events ${r.heard_events}`).join("; "));
    // F4: an SNR appears once something was decoded.
    const [sa, sb] = [await snap("a"), await snap("b")];
    check(
      "F4: no telemetry SNR until a decode, then one",
      report.f4_ok_idle && sa.telemetry.snrDb?.source === "mercury telemetry" && sb.telemetry.snrDb?.source === "mercury telemetry",
      `before any decode (+${report.f4_before_decode.at_s} s): wire snr ${report.f4_before_decode.wire_a.snr} (${report.f4_before_decode.wire_a.snr_example}) with freq_offset_age_ms ${report.f4_before_decode.wire_a.age} on A, ${report.f4_before_decode.wire_b.age} on B; app snrDb A ${JSON.stringify(report.f4_before_decode.app_a)}, B ${JSON.stringify(report.f4_before_decode.app_b)}; after the CQs: A ${JSON.stringify(sa.telemetry.snrDb)}, B ${JSON.stringify(sb.telemetry.snrDb)}`,
    );
    const both = cqRuns.filter((r) => r.order === "MONITOR first");
    const snrInMonFirst = both.every((r) => r.row?.snr?.source === "modem monitor");
    report.questions.q3_summary = `${cqRuns.filter((r) => r.order === "CQFRAME first").length} CQFRAME-first, ${both.length} MONITOR-first; MONITOR-first rows with the SNR: ${both.length ? snrInMonFirst : "n/a"}`;
    const tx = cqRuns.filter((r) => r.final === "transmitted").length;
    check("CQ outcome reaches transmitted (PENDING/CANCELPENDING on the sender)", tx === cqRuns.length, cqRuns.map((r) => `${r.from}#${r.i + 1} ${r.final}`).join(", "));
  }

  // ---- Beacon A -> B with the locator (and the extra KISS client on B's port)
  {
    const t = Date.now();
    await must("a", { type: "setBeaconContent", includeGrid: true });
    const r = await cmd("a", { type: "sendBeaconNow" });
    const h = r.err ? null : await until(() => evs("b", "heard", t).find((e) => e.event.station.callsign === ST.a.call && e.event.station.source === "beacon"), 40000, "the beacon heard").catch(() => null);
    await sleep(2000);
    await pull();
    const outs = evs("a", "broadcast", t).filter((e) => e.event.kind === "beacon").map((e) => `${rel(e.t)} ${e.event.outcome.type}`);
    const bcA = relays.find((x) => x.name === "a-bc").entries.filter((e) => e.t >= t && e.dir === "app>modem");
    const bcB = relays.find((x) => x.name === "b-bc").entries.filter((e) => e.t >= t && e.dir === "modem>app");
    const extraGot = extra.chunks.filter((c) => c.t >= t);
    report.questions.q5_second_broadcast_client = {
      extra_connected: extra.connected != null,
      extra_closed_by_modem: extra.closed != null,
      extra_error: extra.error,
      modem_b_log_on_connect: extra.modem_log,
      beacon_frames_to_app_b: bcB.map((e) => `${rel(e.t)} ${e.n} B`),
      beacon_frames_to_extra: extraGot.map((c) => `${rel(c.t)} ${c.n} B ${c.head}`),
      same_bytes: bcB[0] && extraGot[0] ? bcB[0].head === extraGot[0].head : null,
    };
    const st = h?.event.station;
    check(
      "beacon heard with its locator",
      !!st && st.grid?.toLowerCase() === ST.a.grid.toLowerCase() && st.source === "beacon",
      r.err ? `refused: ${JSON.stringify(r.err)}` : `B heard ${st?.callsign} source ${st?.source} grid ${st?.grid} caps ${st?.caps} after ${h ? ((h.t - t) / 1000).toFixed(1) : "-"} s; A outcomes ${outs.join(", ")}; A wrote ${bcA.map((e) => `${e.n} B`).join(", ")} on its broadcast port`,
    );
  }

  // ---- Session 1: the beacon gate, telemetry loss, LISTEN OFF mid-session (Q2), then LISTEN OFF after DISCONNECTED (Q1, dirty case)
  const RA = relays.find((r) => r.name === "a-ctl");
  const RB = relays.find((r) => r.name === "b-ctl");
  const BCA = relays.find((r) => r.name === "a-bc");
  let sessionEnd1 = null;
  let armedDue = null;
  {
    await must("a", { type: "setBeaconInterval", minutes: BEACON_MIN });
    await must("a", { type: "setBeacon", on: true });
    const armedAt = Date.now();
    const bs = (await snap("a")).beacon;
    armedDue = Date.parse(bs.nextDueUtc);
    report.timings.beacon_armed_s = rel(armedAt);
    report.timings.beacon_first_due_s = rel(armedDue);
    log(`beacons armed: interval ${bs.intervalMin} min, first due in ${((armedDue - armedAt) / 1000).toFixed(0)} s`);

    const c = await callUp("a", "b");
    report.timings.session1 = { call_s: rel(c.t), connected_after_s: +((c.up - c.t) / 1000).toFixed(1) };
    const sessStart = c.up;
    log(`session 1 up after ${((c.up - c.t) / 1000).toFixed(1)} s`);
    if (c.up > armedDue - 20000) report.notes.push("session 1 came up less than 20 s before the beacon was due: the gate window is short");

    // The beacon toggle, Beacon now and CQ during the session.
    const toggle = await cmd("a", { type: "setBeacon", on: true });
    const now1 = await cmd("a", { type: "sendBeaconNow" });
    const cq1 = await cmd("a", { type: "sendCq" });
    const heardBefore = (await heard("b")).find((x) => x.callsign === ST.a.call);

    // Chat, then telemetry relay closed mid-session, then chat both ways.
    const chat1 = await chat("a", "b", "m3 lab: before the telemetry cut");
    const killAt = Date.now();
    await wsRelay.close();
    const lost = await until(async () => {
      const t = (await snap("b")).telemetry;
      return t.link.type === "lost" ? t : null;
    }, 30000, "B's telemetry lost").catch(() => null);
    const lostAfter = lost ? (Date.now() - killAt) / 1000 : null;
    const chat2 = await chat("a", "b", "m3 lab: after the telemetry cut, A to B").catch((e) => `FAIL ${e.message}`);
    const chat3 = await chat("b", "a", "m3 lab: after the telemetry cut, B to A").catch((e) => `FAIL ${e.message}`);
    const sb = await snap("b");
    const stillSession = sb.session?.sessionId === c.peerSessionId && (await snap("a")).session?.sessionId === c.sessionId;
    check(
      "telemetry lost mid-session while chat keeps delivering",
      !!lost && typeof chat2 === "number" && typeof chat3 === "number" && stillSession && sb.telemetry.link.type === "lost" && (await snap("a")).telemetry.link.type === "up",
      `relay closed at +${rel(killAt)} s; B telemetry ${lost ? `lost after ${lostAfter.toFixed(1)} s (${lost.link.reason})` : "never lost"}; chat A->B ${typeof chat2 === "number" ? `${chat2.toFixed(1)} s` : chat2}, B->A ${typeof chat3 === "number" ? `${chat3.toFixed(1)} s` : chat3}; same session ${stillSession}; B link at the end ${sb.telemetry.link.type}; A's telemetry still up; before the cut A->B took ${chat1.toFixed(1)} s`,
    );
    report.timings.telemetry = { relay_closed_s: rel(killAt), lost_after_s: lostAfter, chat_before_s: chat1, chat_after_ab_s: chat2, chat_after_ba_s: chat3, b_view_at_end: sb.telemetry };

    // Hold the session until the beacon has fallen due inside it.
    const holdUntil = Math.max(Date.now(), armedDue + 20000);
    log(`holding session 1 for ${((holdUntil - Date.now()) / 1000).toFixed(0)} s so the beacon falls due inside it`);
    while (Date.now() < holdUntil) {
      await pull();
      await sleep(2000);
    }
    const bsIn = (await snap("a")).beacon;
    sessionEnd1 = await endSession("a", "b", "session 1: A answering calls (no LISTEN change), A ends");
    report.questions.q1_sessions = [sessionEnd1];
    log(`session 1 ended by A: B saw DISCONNECTED after ${sessionEnd1.peer_disconnected_after_s} s`);
    const sessEnd = T0 + sessionEnd1.disconnect_sent_s * 1000;
    const inWindow = (t) => t >= sessStart && t <= sessEnd;
    const bcWritten = BCA.entries.filter((e) => e.dir === "app>modem" && inWindow(e.t));
    const modemQueued = modemLines("a", /Broadcast frame queued|CQ transmission started|Queued CQ/, sessStart, sessEnd);
    const heardAfter = (await heard("b")).find((x) => x.callsign === ST.a.call);
    const bBeaconsDuring = evs("b", "heard", sessStart).filter((e) => e.event.station.source === "beacon" && e.t <= sessEnd);
    const extraDuring = extra.chunks.filter((ch) => inWindow(ch.t));
    const gateOk = !!toggle.err && !!now1.err && !!cq1.err && bcWritten.length === 0 && modemQueued.length === 0 && bBeaconsDuring.length === 0 && extraDuring.length === 0 && armedDue < sessEnd && bsIn.nextDueUtc === null;
    check(
      "session gate: no broadcast during the session",
      gateOk,
      `during the session: setBeacon on -> ${JSON.stringify(toggle.err?.message ?? toggle.ok)}; Beacon now -> ${JSON.stringify(now1.err?.message ?? now1.ok)}; CQ -> ${JSON.stringify(cq1.err?.message ?? cq1.ok)}; ` +
        `armed beacon fell due at +${rel(armedDue)} s inside the session (+${rel(sessStart)}..+${sessionEnd1.disconnect_sent_s} s), status then: nextDue ${bsIn.nextDueUtc}, suppressed "${bsIn.suppressed}"; ` +
        `A's app wrote ${bcWritten.length} frames to its broadcast port, A's modem log ${modemQueued.length} broadcast/CQ queue lines, B heard ${bBeaconsDuring.length} beacons, the extra KISS client got ${extraDuring.length} chunks; B's row for A ${heardBefore?.source}/${heardBefore?.count} -> ${heardAfter?.source}/${heardAfter?.count}`,
    );
    report.timings.session1.gate = { armed_due_s: rel(armedDue), status_in_session: bsIn };
  }

  // ---- After the session: exactly one beacon, one jittered interval after idle
  {
    // The link's end as A's modem reported it (control relay): the app goes
    // idle at that moment, and the next beacon is one jittered interval on.
    const idleAt = sessionEnd1.ender_disconnected_t;
    const bs = (await snap("a")).beacon;
    const due = bs.nextDueUtc ? Date.parse(bs.nextDueUtc) : null;
    log(`after session 1: A idle at +${rel(idleAt)} s; next beacon due ${due ? `in ${((due - idleAt) / 1000).toFixed(0)} s after idle` : "never"}`);
    const first = await until(() => evs("a", "broadcast", idleAt).find((e) => e.event.kind === "beacon" && e.event.outcome.type === "submitted"), BEACON_MIN * 60 * 1150 + 30000, "the post-session beacon").catch(() => null);
    // Watch on for 60 s for a second one (the next is at least 0.85 x interval away).
    const watchEnd = Date.now() + 60000;
    while (Date.now() < watchEnd) {
      await pull();
      await sleep(2000);
    }
    const sent = evs("a", "broadcast", idleAt).filter((e) => e.event.kind === "beacon" && e.event.outcome.type === "submitted");
    const queued = modemLines("a", /Broadcast frame queued/, idleAt, Date.now());
    const heardB = evs("b", "heard", idleAt).filter((e) => e.event.station.callsign === ST.a.call && e.event.station.source === "beacon");
    // When the beacon frame reached A's broadcast port (relay), not when the event was polled.
    const wrote = BCA.entries.find((e) => e.dir === "app>modem" && e.t >= idleAt);
    const after = wrote ? (wrote.t - idleAt) / 1000 : first ? (first.t - idleAt) / 1000 : null;
    const lo = BEACON_MIN * 60 * 0.85 - 2;
    const hi = BEACON_MIN * 60 * 1.15 + 5;
    check(
      "exactly one beacon after the session, one interval later",
      sent.length === 1 && queued.length === 1 && heardB.length === 1 && after >= lo && after <= hi,
      `interval ${BEACON_MIN} min; A idle at +${rel(idleAt)} s; beacons submitted ${sent.length} (${sent.map((e) => `+${rel(e.t)} s`).join(", ")}), ${after == null ? "" : `${after.toFixed(1)} s after idle (allowed ${lo.toFixed(0)}..${hi.toFixed(0)} s); `}A's modem queued ${queued.length}; B heard ${heardB.length} with grid ${heardB[0]?.event.station.grid}; watched until +${rel(Date.now())} s`,
    );
    report.timings.post_session_beacon = { idle_s: rel(idleAt), due_s: rel(due), sent_s: sent.map((e) => rel(e.t)), after_idle_s: after, heard_by_b_s: heardB.map((e) => rel(e.t)) };
    await must("a", { type: "setBeacon", on: false });
  }

  // ---- Q1: LISTEN OFF straight after DISCONNECTED. Receive only chosen while
  // the call rings (the modem task holds it until the link ends), so the app's
  // first idle reconcile after the link is a LISTEN OFF; each side ends once.
  // Then the same with A in Receive only throughout (no LISTEN after the link).
  const q1 = report.questions.q1_sessions;
  for (const [label, ender, toggle] of [
    ["session 2: Receive only chosen while A's call rang, A ends", "a", true],
    ["session 3: Receive only chosen while A's call rang, B ends", "b", true],
    ["session 4: A in Receive only throughout, A ends", "a", false],
    ["session 5: A in Receive only throughout, B ends", "b", false],
  ]) {
    try {
      await must("a", { type: "setReceiveOnly", on: !toggle });
      await until(async () => (await snap("a")).listening === toggle, 15000, "A's listening to follow Receive only");
      const s = await callUp("a", "b", 300000, toggle ? async () => {
        await until(async () => (await snap("a")).modem.type === "calling", 10000, "A calling");
        await must("a", { type: "setReceiveOnly", on: true });
      } : null);
      await chat("a", "b", `m3 lab: ${label}`);
      const e = await endSession(ender, ender === "a" ? "b" : "a", label);
      e.connected_after_s = +((s.up - s.t) / 1000).toFixed(1);
      q1.push(e);
      log(`Q1 ${label}: other side saw the end after ${e.peer_disconnected_after_s} s; A sent after the link: ${(ender === "a" ? e.ender_listen_after : e.peer_listen_after).join(" | ") || "nothing"}`);
    } catch (err) {
      q1.push({ label, error: err.message });
      log(`Q1 ${label}: ${err.message}`);
    }
  }
  {
    const strands = q1.filter((e) => e.peer_disconnected_after_s != null);
    const worst = Math.max(...strands.map((e) => e.peer_disconnected_after_s));
    check(
      "a session end reaches the other side promptly (no stranding)",
      strands.length === q1.length && worst < 60,
      q1.map((e) => (e.error ? `${e.label}: ${e.error}` : `${e.label}: other side DISCONNECTED ${e.peer_disconnected_after_s} s after DISCONNECT`)).join("; "),
    );
  }

  // ---- Q2 / F2: Receive only switched on mid-session (A answering, A called B).
  // The live link is never touched: the choice is pending until the session
  // ends, chat keeps going both ways, and LISTEN OFF follows A's DISCONNECTED.
  {
    const name = "Receive only mid-session waits for the session to end (F2)";
    try {
      await must("a", { type: "setReceiveOnly", on: false });
      await until(async () => (await snap("a")).listening, 15000, "A listening again");
      const s = await callUp("a", "b");
      await chat("a", "b", "m3 lab: session 6, before Receive only");
      const roAt = Date.now();
      await must("a", { type: "setReceiveOnly", on: true });
      const pending = await until(async () => {
        const sa = await snap("a");
        return sa.receiveOnly && sa.receiveOnlyPending ? sa : null;
      }, 5000, "the pending state in the snapshot").catch(() => null);
      const header = await A.sess.eval("document.querySelector('.station-mode .mode-word')?.textContent.trim() ?? null");
      const ev = evs("a", "receiveOnly", roAt).map((e) => `${rel(e.t)} on ${e.event.on} pending ${e.event.pending}`);
      // Still linked a while later, and chat both ways.
      await sleep(15000);
      const ab = await chat("a", "b", "m3 lab: session 6, Receive only pending, A to B").catch((e) => `FAIL ${e.message}`);
      const ba = await chat("b", "a", "m3 lab: session 6, Receive only pending, B to A").catch((e) => `FAIL ${e.message}`);
      const [sa1, sb1] = [await snap("a"), await snap("b")];
      const sameLink = sa1.session?.sessionId === s.sessionId && sb1.session?.sessionId === s.peerSessionId;
      const listenWhileLinked = lines(RA, roAt, Date.now(), /^LISTEN /).filter((e) => e.dir === "app>modem");
      const dropped = modemLines("a", /LISTEN OFF on a live link|reason=listen_off/, roAt);
      // Now end it gracefully from A.
      const e = await endSession("a", "b", "session 6: Receive only switched on mid-session, then A ends");
      const aDisc = e.ender_disconnected_t;
      const listenAfter = lines(RA, roAt, Date.now(), /^LISTEN /).filter((x) => x.dir === "app>modem");
      const sa2 = await snap("a");
      const q = {
        connected_after_s: +((s.up - s.t) / 1000).toFixed(1),
        set_at_s: rel(roAt),
        snapshot_pending: pending ? { receiveOnly: pending.receiveOnly, receiveOnlyPending: pending.receiveOnlyPending, listening: pending.listening, modem: pending.modem.type } : null,
        header,
        receive_only_events: evs("a", "receiveOnly", roAt).map((x) => `${rel(x.t)} on ${x.event.on} pending ${x.event.pending}`),
        chat_ab_s: ab,
        chat_ba_s: ba,
        same_link: sameLink,
        listen_while_linked: show(listenWhileLinked),
        modem_dropped_link: dropped.map((l) => `${rel(l.t)} ${l.text}`),
        end: e,
        listen_total: show(listenAfter),
        listen_after_disconnected_ms: listenAfter.filter((x) => x.t >= aDisc).map((x) => x.t - aDisc),
        after: { receiveOnly: sa2.receiveOnly, receiveOnlyPending: sa2.receiveOnlyPending, listening: sa2.listening, modem: sa2.modem.type },
      };
      report.questions.q2_listen_off_mid_session = q;
      log(`Q2: pending ${!!pending}, header "${header}", LISTEN while linked ${listenWhileLinked.length}, chat ${ab}/${ba}; after DISCONNECT: LISTEN ${q.listen_total.join(" | ")}, B saw the end after ${e.peer_disconnected_after_s} s`);
      const ok =
        !!pending &&
        /after this session/i.test(header ?? "") &&
        listenWhileLinked.length === 0 &&
        dropped.length === 0 &&
        typeof ab === "number" &&
        typeof ba === "number" &&
        sameLink &&
        listenAfter.length === 1 &&
        /^LISTEN OFF$/.test(listenAfter[0].line) &&
        listenAfter[0].t >= aDisc &&
        e.peer_disconnected_after_s != null &&
        e.peer_disconnected_after_s < 30 &&
        sa2.receiveOnly &&
        !sa2.receiveOnlyPending &&
        !sa2.listening;
      check(
        name,
        ok,
        `snapshot receiveOnly ${pending?.receiveOnly}, pending ${pending?.receiveOnlyPending} (events ${ev.join(", ") || q.receive_only_events.join(", ")}); header "${header}"; ` +
          `no LISTEN while linked (${listenWhileLinked.length}), Mercury dropped the link: ${dropped.length ? "yes" : "no"}; chat A->B ${typeof ab === "number" ? `${ab.toFixed(1)} s` : ab}, B->A ${typeof ba === "number" ? `${ba.toFixed(1)} s` : ba}, same session ${sameLink}; ` +
          `A's DISCONNECT, then ${q.listen_total.length} LISTEN (${q.listen_total.join(" | ")}), ${q.listen_after_disconnected_ms.map((m) => `+${m} ms`).join(", ")} after A's DISCONNECTED; B's DISCONNECTED ${e.peer_disconnected_after_s} s after DISCONNECT; then receiveOnly ${sa2.receiveOnly}, pending ${sa2.receiveOnlyPending}, listening ${sa2.listening}`,
      );
    } catch (err) {
      check(name, false, err.message);
    }
    // Back to answering calls for anything after this.
    await cmd("a", { type: "setReceiveOnly", on: false });
    await until(async () => (await snap("a")).listening, 15000, "A answering calls again").catch(() => null);
  }

  // ---- QSO draft and heard rows: no frequency without a rig
  {
    const sa = await snap("a");
    const q = (await must("a", { type: "qsos", limit: 10 })).qsos;
    const rows = await heard("b");
    const ok = sa.currentQso && sa.currentQso.freq === null && q.every((x) => x.freq === null) && rows.every((r) => r.freq === null && r.band === null);
    check(
      "frequency unavailable in QSO drafts and heard rows",
      !!ok,
      `current QSO ${sa.currentQso?.peer} ${sa.currentQso?.state} freq ${JSON.stringify(sa.currentQso?.freq)}; ${q.length} QSO rows, freq ${[...new Set(q.map((x) => JSON.stringify(x.freq)))].join()}; B's heard rows freq ${[...new Set(rows.map((r) => JSON.stringify(r.freq)))].join()}`,
    );
    report.qsos_a = q;
  }

  // ---- Q6 again, with traffic seen: the offset and SNR as written while decoding
  report.questions.q6_telemetry_ws.forms_a_end = wsA.forms;
  report.questions.q6_telemetry_ws.forms_b_end = wsB.forms;
  report.questions.q6_telemetry_ws.status_with_offset_a = wsA.samples[1] ?? null;
  report.questions.q6_telemetry_ws.app_view_a_end = (await snap("a")).telemetry;
} catch (e) {
  check("run", false, e.stack ?? e.message);
} finally {
  clearInterval(pump);
  try {
    await pull();
    for (const id of Object.keys(inst)) {
      report.activity[id] = (await must(id, { type: "activity" })).entries;
      const ui = await inst[id].sess.eval("({ wrapped: window.__lab.wrapped, ui: window.__lab.ui, err: window.__lab.err })");
      report.ui_commands ??= {};
      report.ui_commands[id] = { ...ui, ui: ui.ui.map((u) => ({ s: rel(u.t), cmd: u.cmd })) };
      if (ui.ui.length) check(`no commands from window ${id} itself`, false, `the window sent ${ui.ui.map((u) => `+${rel(u.t)} s ${u.cmd}`).join("; ")} (someone used it during the run)`);
      report.events[id] = inst[id].events;
    }
  } catch {}
  wsA?.close();
  wsB?.close();
  for (const r of relays) report.relays[r.name] = { listen: r.listen, target: r.target, conns: r.conns, bytes: r.bytes, entries: r.entries.map((e) => ({ s: rel(e.t), ...e })) };
  cleanup();
  await sleep(1500);
  report.finished_utc = iso(Date.now());
  const pass = results.filter((r) => r.pass).length;
  writeFileSync(join(OUT, "report.json"), JSON.stringify(report, null, 1));
  const md = [
    "# M3a discovery over two Mercury modems (no RF)",
    "",
    `${pass} passed, ${results.length - pass} failed. Started ${report.started_utc}, finished ${report.finished_utc}.`,
    "",
    `Mercury ${report.manifest.mercury_sha256}; app ${report.manifest.app_sha256}.`,
    "",
    "| Check | Result | Detail |",
    "|---|---|---|",
    ...results.map((r) => `| ${r.name} | ${r.pass ? "PASS" : "FAIL"} | ${String(r.detail).replace(/\|/g, "\\|").replace(/\n/g, " ")} |`),
    "",
    "## Questions",
    "",
    "```json",
    JSON.stringify({ questions: report.questions, timings: report.timings, notes: report.notes }, null, 1),
    "```",
  ].join("\n");
  writeFileSync(join(OUT, "report.md"), md + "\n");
  console.log(`${pass} passed, ${results.length - pass} failed; report in ${OUT}`);
  process.exit(0);
}
