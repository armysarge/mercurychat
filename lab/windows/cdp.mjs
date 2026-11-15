// Shared helpers for the lab window checks: launch the desktop build with a
// WebView2 DevTools port, find its page, and evaluate this script's own fixed
// expressions in it (CDP Runtime.evaluate in the lab instance it launched).
import { spawn, execFileSync } from "node:child_process";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function launch(exe, dataDir, port) {
  const env = {
    ...process.env,
    MC_DATA_DIR: dataDir,
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

async function pages(port) {
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
