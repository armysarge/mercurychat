// M5 responsive check (owner, 23 Nov: "100% responsiveness up to at least
// tablet size resolutions"): the real window at tablet portrait 768×1024,
// tablet landscape 1024×768 and desktop 1366×768 (responsive.mjs measures).
// Views: the plain setup screen, Operate, and Settings' Profiles, Modem, Rig
// and Audio tabs; the wizard's steps are measured by m5-wizard-check.mjs. A
// debug build with a FAKE bundle (as m5-ui-check.mjs): no Mercury, no audio
// device, no rig, no RF.
// Usage: node lab/windows/m5-responsive.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { firstUse, kill, launch, sleep, until } from "./cdp.mjs";
import { pill, setInput } from "./m2-dom.mjs";
import { shoot as measure } from "./responsive.mjs";

const [exe, dataDir, outDir, port = "9364"] = process.argv.slice(2);
if (!exe || !dataDir || !outDir) {
  console.error("usage: node m5-responsive.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]");
  process.exit(2);
}
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;

function fakeBundle() {
  const bin = dirname(exe);
  const dir = `${dataDir}-bundle`;
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  let manifest = "";
  for (const [from, to] of [["fake-mercury.exe", "mercury.exe"], ["fake-rigctld.exe", "rigctld.exe"]]) {
    copyFileSync(join(bin, from), join(dir, to));
    manifest += `${createHash("sha256").update(readFileSync(join(dir, to))).digest("hex")}  ${to}\n`;
  }
  writeFileSync(join(dir, "MANIFEST.sha256"), manifest);
  writeFileSync(join(dir, "BUILD.txt"), "hamlib_version=4.7.2\nmercury_version=v1.9.25\n");
  return dir;
}

const results = [];
let s;
let app;
const shoot = async (view) => results.push(...(await measure(s, outDir, view)));

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  process.env.MC_BUNDLE_DIR = fakeBundle();
  app = launch(exe, dataDir, port);
  delete process.env.MC_BUNDLE_DIR;
  s = await firstUse(port, "Built-in Mercury", 40000);
  await shoot("first-use");

  await s.eval(setInput('form.builtin input[name="callsign"]', "ZS6RSP"));
  await s.eval("document.querySelector('form.builtin details.advanced').open = true");
  for (const [n, v] of [["control", 18400], ["broadcast", 18200], ["ui", 20100], ["rigctld", 14632]]) {
    await s.eval(setInput(`form.builtin input[name="${n}"]`, String(v)));
  }
  await s.eval("document.querySelector('form.builtin button[type=submit]').click()");
  await sleep(1500);
  if (await s.eval("!!document.querySelector('form.builtin button.use-free')")) {
    await s.eval("document.querySelector('form.builtin button.use-free').click()");
  }
  await until(s, `['ready','listening'].includes(${pill})`, "the built-in modem ready", 40000);
  await shoot("operate");
  for (const tab of ["profiles", "modem", "rig", "audio"]) {
    await s.eval(`${rail("Settings")}.click()`);
    await until(s, `!!document.getElementById('tab-${tab}')`, "Settings");
    await s.eval(`document.getElementById('tab-${tab}').click()`);
    await sleep(500);
    await shoot(`settings-${tab}`);
  }
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  s?.close();
  kill(app);
}
for (const r of results) console.log(r);
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
