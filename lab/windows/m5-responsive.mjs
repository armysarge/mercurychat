// M5 responsive check (owner, 23 Nov: "100% responsiveness up to at least
// tablet size resolutions"): the real window at tablet sizes, portrait
// 768×1024 and landscape 1024×768, and the desktop 1366×768. For each view it
// saves a screenshot and fails when the page scrolls sideways (anything wider
// than the window). A debug build with a FAKE bundle (as m5-ui-check.mjs):
// no Mercury, no audio device, no rig, no RF.
// Usage: node lab/windows/m5-responsive.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { kill, launch, page, sleep, until } from "./cdp.mjs";
import { pill, setInput } from "./m2-dom.mjs";

const [exe, dataDir, outDir, port = "9364"] = process.argv.slice(2);
if (!exe || !dataDir || !outDir) {
  console.error("usage: node m5-responsive.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]");
  process.exit(2);
}
const SIZES = [
  ["tablet-portrait", 768, 1024],
  ["tablet-landscape", 1024, 768],
  ["desktop", 1366, 768],
];
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

/** Screenshots of the current view at every size; a sideways scroll fails it. */
async function shoot(view) {
  for (const [name, w, h] of SIZES) {
    await s.send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await sleep(600);
    const over = await s.eval(`(() => {
      const de = document.documentElement;
      // Visible and not inside its own scroller (a wide table in an
      // overflow-x box is fine; a closed drawer is visibility: hidden).
      const clipped = (e) => {
        for (let p = e.parentElement; p && p !== document.body; p = p.parentElement) {
          const ox = getComputedStyle(p).overflowX;
          if (ox === 'auto' || ox === 'scroll' || ox === 'hidden' || ox === 'clip') {
            if (p.getBoundingClientRect().right <= innerWidth + 1) return true;
          }
        }
        return false;
      };
      const wide = [...document.querySelectorAll('body *')].filter(e => {
        const r = e.getBoundingClientRect();
        return r.width > 0 && r.right > innerWidth + 1 && getComputedStyle(e).visibility !== 'hidden' && !clipped(e);
      }).slice(0, 5).map(e => e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') + (e.className && typeof e.className === 'string' ? '.' + e.className.split(' ')[0] : '') + '@' + Math.round(e.getBoundingClientRect().right));
      const scroll = de.scrollWidth - innerWidth;
      // Which boxes scroll sideways, to name the cause of a page scroll.
      if (scroll > 0 && wide.length === 0) {
        for (const e of [document.body, ...document.querySelectorAll('body *')]) {
          if (e.scrollWidth > e.clientWidth + 1 && getComputedStyle(e).overflowX !== 'visible' && e.clientWidth > 0) {
            wide.push('overflowing ' + e.tagName.toLowerCase() + (e.className && typeof e.className === 'string' ? '.' + e.className.split(' ')[0] : '') + ' ' + e.scrollWidth + '>' + e.clientWidth);
            if (wide.length >= 5) break;
          }
        }
      }
      return { scroll, wide };
    })()`);
    const shot = await s.send("Page.captureScreenshot", { format: "png" });
    writeFileSync(join(outDir, `${view}-${name}.png`), Buffer.from(shot.data, "base64"));
    const ok = over.scroll <= 0 && over.wide.length === 0;
    results.push(`${ok ? "PASS" : "FAIL"} ${view} ${w}x${h}: sideways ${over.scroll} px${over.wide.length ? `, past the edge: ${over.wide.join(", ")}` : ""}`);
  }
  await s.send("Emulation.clearDeviceMetricsOverride");
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  process.env.MC_BUNDLE_DIR = fakeBundle();
  app = launch(exe, dataDir, port);
  delete process.env.MC_BUNDLE_DIR;
  s = await page(port, (t) => t.includes("Built-in Mercury"), "the first-use screen", 40000);
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
