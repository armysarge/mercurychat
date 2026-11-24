// Shared helpers for the lab window checks: launch the desktop build with a
// WebView2 DevTools port, find its page, and evaluate this script's own fixed
// expressions in it (CDP Runtime.evaluate in the lab instance it launched).
import { spawn, execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function launch(exe, dataDir, port) {
  const env = {
    ...process.env,
    MC_DATA_DIR: dataDir,
    // A lab WebView2 profile per DevTools port: otherwise a MercuryChat the
    // operator has open owns the shared browser process and this instance gets
    // no DevTools port. Kept between runs, because a brand-new profile delays
    // the first page's script by seconds.
    WEBVIEW2_USER_DATA_FOLDER: join(tmpdir(), `mc-lab-webview2-${port}`),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
  };
  const child = spawn(exe, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  child.out = "";
  child.stdout.on("data", (d) => (child.out += d));
  child.stderr.on("data", (d) => (child.out += d));
  return child;
}

export function kill(child) {
  try {
    execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
  } catch {}
}

export async function pages(port) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/list`);
    return (await r.json()).filter((t) => t.type === "page");
  } catch {
    return [];
  }
}

export function connect(wsUrl) {
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
    /** Any DevTools method (e.g. Emulation.setDeviceMetricsOverride). */
    async send(method, params = {}) {
      const n = ++id;
      const reply = new Promise((r) => waiting.set(n, r));
      ws.send(JSON.stringify({ id: n, method, params }));
      const msg = await reply;
      if (msg.error) throw new Error(`${method}: ${msg.error.message}`);
      return msg.result;
    },
    async eval(expr) {
      const n = ++id;
      const reply = new Promise((r) => waiting.set(n, r));
      ws.send(JSON.stringify({ id: n, method: "Runtime.evaluate", params: { expression: expr, awaitPromise: true, returnByValue: true } }));
      const msg = await reply;
      if (msg.result?.exceptionDetails) throw new Error(JSON.stringify(msg.result.exceptionDetails) + " EXPR: " + String(expr).slice(0, 300));
      return msg.result?.result?.value;
    },
    close: () => ws.close(),
  }));
}

/** Wait for a page whose text satisfies `test`; returns a connected session. */
export async function page(port, test, what, ms = 30000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    for (const t of await pages(port)) {
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

export async function until(s, expr, what, ms = 10000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const v = await s.eval(expr);
    if (v) return v;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}: ${await s.eval("document.body.innerText")}`);
}

/** The plain first-use screen (M5): a new install opens the setup wizard;
 *  "Set up without the guide" leads to the screen the older checks drive.
 *  Waits for `text` on it. */
export async function firstUse(port, text, ms = 40000) {
  const s = await page(port, (t) => t.includes(text) || t.includes("Set up without the guide"), "the first-use screen", ms);
  await s.eval("document.querySelector('button.skip-wizard')?.click()");
  await until(s, `document.body.innerText.includes(${JSON.stringify(text)})`, "the first-use screen", 15000);
  return s;
}
