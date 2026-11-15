// M1 Task 9 end-to-end check of the real desktop window, driven over the
// WebView2 DevTools protocol (no RF, simulator only):
//   1. first instance: "Use simulator" for a callsign -> the modem shows ready
//   2. second instance on the same data root -> the window shows the busy text
//   3. restart -> the profile is still there, still in use, and ready again
// Usage: node lab/windows/m1-ui-check.mjs <mercurychat.exe> <data dir> [port]
import { spawn, execFileSync } from "node:child_process";

const [exe, dataDir, port = "9333"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m1-ui-check.mjs <mercurychat.exe> <data dir> [port]");
  process.exit(2);
}
const CALL = "ZS6SIM";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function launch() {
  const env = {
    ...process.env,
    MC_DATA_DIR: dataDir,
    // Both instances share one WebView2 user-data folder, so both get the same arguments.
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
  };
  const child = spawn(exe, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  child.out = "";
  child.stdout.on("data", (d) => (child.out += d));
  child.stderr.on("data", (d) => (child.out += d));
  return child;
}

function kill(child) {
  try {
    execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
  } catch {}
}

async function pages() {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/list`);
    return (await r.json()).filter((t) => t.type === "page");
  } catch {
    return [];
  }
}

function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let id = 0;
  const waiting = new Map();
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && waiting.has(msg.id)) {
      waiting.get(msg.id)(msg);
      waiting.delete(msg.id);
    }
  };
  const ready = new Promise((r, j) => {
    ws.onopen = r;
    ws.onerror = j;
  });
  return ready.then(() => ({
    // CDP Runtime.evaluate of this script's own fixed expressions, in the
    // lab instance it launched (not JavaScript eval of outside input).
    async eval(expr) {
      const n = ++id;
      const reply = new Promise((r) => waiting.set(n, r));
      ws.send(JSON.stringify({ id: n, method: "Runtime.evaluate", params: { expression: expr, awaitPromise: true, returnByValue: true } }));
      const msg = await reply;
      if (msg.result?.exceptionDetails) throw new Error(JSON.stringify(msg.result.exceptionDetails));
      return msg.result?.result?.value;
    },
    close: () => ws.close(),
  }));
}

/** Wait for a page whose text satisfies `test`; returns a connected session. */
async function page(test, what, ms = 30000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    for (const t of await pages()) {
      try {
        const s = await connect(t.webSocketDebuggerUrl);
        const text = await s.eval("document.body ? document.body.innerText : ''");
        if (test(text)) return s;
        s.close();
      } catch {}
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${what}`);
}

async function until(s, expr, what, ms = 10000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const v = await s.eval(expr);
    if (v) return v;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}: ${await s.eval("document.body.innerText")}`);
}

const pill = "document.querySelector('.pill')?.textContent.trim()";
const detail = "document.querySelector('.state .detail')?.textContent.trim()";
const results = [];
let first, second, third;
try {
  first = launch();
  let s = await page((t) => t.includes("Use simulator"), "the first window");
  await s.eval(`(() => {
    const i = document.querySelector('input[maxlength="15"]');
    i.value = ${JSON.stringify(CALL.toLowerCase())};
    i.dispatchEvent(new Event('input'));
    [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Use simulator').click();
  })()`);
  await until(s, `${pill} === 'ready' && ${detail} === 'simulator' && document.body.innerText.includes(${JSON.stringify(CALL)})`, "ready");
  results.push(`PASS select: ${CALL} simulator profile saved and selected, modem state "ready", detail "simulator"`);
  s.close();

  second = launch();
  s = await page((t) => t.includes("cannot start"), "the second window's busy message");
  const busy = await s.eval("document.querySelector('[role=alert]').innerText");
  if (!busy.includes("another MercuryChat instance")) throw new Error(`unexpected busy text: ${busy}`);
  results.push(`PASS busy: second instance shows "${busy.split("\n").find((l) => l.includes("another"))}"`);
  s.close();
  kill(second);
  kill(first);
  await sleep(1500);

  third = launch();
  s = await page((t) => t.includes("Use simulator") && t.includes(CALL), "the restarted window");
  await until(s, `${pill} === 'ready' && ${detail} === 'simulator' && document.querySelector('.profiles li.active')?.innerText.includes(${JSON.stringify(CALL)})`, "ready after restart");
  results.push(`PASS restart: ${CALL} still listed and in use, simulator re-attached, modem state "ready", detail "simulator"`);
  s.close();
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  for (const c of [first, second, third]) if (c) kill(c);
}
console.log(results.join("\n"));
if (first?.out) console.log("--- first instance output ---\n" + first.out.trim());
if (second?.out) console.log("--- second instance output ---\n" + second.out.trim());
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
