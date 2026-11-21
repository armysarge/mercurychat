// M3b.1 Task 9: MercuryChat stops its own rigctld when the window quits.
// The lab found it left running after the app exited (RigctldDaemon's
// kill-on-drop never ran: the process ended first); the app now asks the core
// to shut down on exit. Windows, the real app, no radio and no Hamlib: the
// station's Hamlib folder holds fake-rigctld renamed rigctld.exe, and its
// modem points at nothing (127.0.0.1:9, so no Mercury is touched).
//
//   1. Frequency control "MercuryChat runs rigctld": the daemon reads "running"
//      and something answers on 127.0.0.1:4532
//   2. the main window closed (WM_CLOSE, as the operator does): within 5 s the app has exited and
//      nothing answers on 4532 any more
//
// Usage: node lab/windows/m3b1-exit-check.mjs <mercurychat.exe> <fake-rigctld.exe> <empty work dir> [port]
// 4532 must be free (the check refuses to start otherwise).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { connect as tcp } from "node:net";
import { join } from "node:path";
import { connect, kill, launch, pages, sleep, until } from "./cdp.mjs";

const [exe, fake, work, port = "9343"] = process.argv.slice(2);
if (!exe || !fake || !work) {
  console.error("usage: node m3b1-exit-check.mjs <mercurychat.exe> <fake-rigctld.exe> <empty work dir> [port]");
  process.exit(2);
}
const answers = (p) =>
  new Promise((ok) => {
    const s = tcp({ host: "127.0.0.1", port: p });
    const done = (v) => {
      s.destroy();
      ok(v);
    };
    s.setTimeout(500, () => done(false));
    s.on("connect", () => done(true));
    s.on("error", () => done(false));
  });

if (await answers(4532)) {
  console.error("something already listens on 127.0.0.1:4532: stop it first");
  process.exit(2);
}
const hamlib = join(work, "hamlib");
mkdirSync(hamlib, { recursive: true });
copyFileSync(fake, join(hamlib, "rigctld.exe"));

const app = launch(exe, join(work, "data"), port);
const exited = new Promise((ok) => app.on("exit", (code) => ok(code)));
let result = 1;
try {
  let main = null;
  for (const end = Date.now() + 60000; !main && Date.now() < end; await sleep(250)) {
    main = (await pages(port)).find((t) => t.url && !t.url.includes("splash.html") && !t.url.startsWith("about:"));
  }
  if (!main) throw new Error("no main window");
  const s = await connect(main.webSocketDebuggerUrl);
  await until(s, "!!window.__TAURI_INTERNALS__", "the Tauri bridge", 20000);
  const call = (cmd) => s.eval(`window.__TAURI_INTERNALS__.invoke('app_call', { cmd: ${JSON.stringify(cmd)} })`);
  const profile = {
    id: "",
    callsign: "N0CALL",
    name: null,
    grid: null,
    lab: false,
    receiveOnly: true,
    modem: { kind: "existing", host: "127.0.0.1", controlPort: 9, dataPort: 10, broadcastPort: null, telemetryUrl: null },
  };
  const saved = await call({ type: "saveProfile", profile });
  await call({ type: "selectProfile", id: saved.id });
  const rig = await call({ type: "rigSettings" });
  const config = { ...rig.config, freqControl: "managedRigctld", hamlibDir: hamlib, cat: { ...rig.config.cat, model: 1, serialPort: "COM250" } };
  await call({ type: "setRigConfig", config });
  await until(s, "(async () => (await window.__TAURI_INTERNALS__.invoke('app_call', { cmd: { type: 'snapshot' } })).rig.daemon === 'running')()", "the managed rigctld running", 15000);
  const up = await answers(4532);
  console.log(`${up ? "PASS" : "FAIL"} managed: daemon "running"; 127.0.0.1:4532 answers ${up}`);
  const t0 = Date.now();
  // As the operator closes it: WM_CLOSE to the main window.
  execFileSync("powershell", ["-NoProfile", "-Command", `(Get-Process -Id ${app.pid}).CloseMainWindow() | Out-Null`]);
  const code = await Promise.race([exited, sleep(5000).then(() => "still running")]);
  const gone = !(await answers(4532));
  const ok = up && code !== "still running" && gone;
  console.log(`${ok ? "PASS" : "FAIL"} quit: app exit ${code} after ${Date.now() - t0} ms; 4532 free ${gone}`);
  result = ok ? 0 : 1;
  if (!ok) console.log(`app output:
${app.out.slice(-2000)}`);
} catch (e) {
  console.log(`FAIL ${e.message}`);
} finally {
  kill(app);
  // A fake left behind by a failed run: stop it by name inside the work dir only.
  if (await answers(4532)) console.log("NOTE rigctld (the fake) still answers on 4532");
}
process.exit(result);
