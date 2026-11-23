// The owner's responsiveness rule (23 Nov: "100% responsiveness up to at
// least tablet size resolutions"), as one measurement the window checks
// share: the view at tablet portrait 768×1024, tablet landscape 1024×768 and
// desktop 1366×768, a screenshot of each, and a failure when the page
// scrolls sideways or a visible element sits past the right edge outside a
// scroller of its own (a wide table in an overflow-x box scrolls on purpose;
// a closed drawer is visibility: hidden).
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { sleep } from "./cdp.mjs";

export const SIZES = [
  ["tablet-portrait", 768, 1024],
  ["tablet-landscape", 1024, 768],
  ["desktop", 1366, 768],
];

const OVERFLOW = `(() => {
  const de = document.documentElement;
  const clipped = (e) => {
    for (let p = e.parentElement; p && p !== document.body; p = p.parentElement) {
      const ox = getComputedStyle(p).overflowX;
      if (ox === 'auto' || ox === 'scroll' || ox === 'hidden' || ox === 'clip') {
        if (p.getBoundingClientRect().right <= innerWidth + 1) return true;
      }
    }
    return false;
  };
  const label = (e) => e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') + (e.className && typeof e.className === 'string' ? '.' + e.className.split(' ')[0] : '');
  const wide = [...document.querySelectorAll('body *')].filter(e => {
    const r = e.getBoundingClientRect();
    return r.width > 0 && r.right > innerWidth + 1 && getComputedStyle(e).visibility !== 'hidden' && !clipped(e);
  }).slice(0, 5).map(e => label(e) + '@' + Math.round(e.getBoundingClientRect().right));
  const scroll = de.scrollWidth - innerWidth;
  // Which boxes scroll sideways, to name the cause of a page scroll.
  if (scroll > 0 && wide.length === 0) {
    for (const e of [document.body, ...document.querySelectorAll('body *')]) {
      if (e.scrollWidth > e.clientWidth + 1 && getComputedStyle(e).overflowX !== 'visible' && e.clientWidth > 0) {
        wide.push('overflowing ' + label(e) + ' ' + e.scrollWidth + '>' + e.clientWidth);
        if (wide.length >= 5) break;
      }
    }
  }
  return { scroll, wide };
})()`;

/** Measure and screenshot the current view at each size; one PASS/FAIL line per size. */
export async function shoot(s, outDir, view, sizes = SIZES) {
  const lines = [];
  for (const [name, w, h] of sizes) {
    await s.send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await sleep(600);
    const over = await s.eval(OVERFLOW);
    if (outDir) {
      const shot = await s.send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(outDir, `${view}-${name}.png`), Buffer.from(shot.data, "base64"));
    }
    const ok = over.scroll <= 0 && over.wide.length === 0;
    lines.push(`${ok ? "PASS" : "FAIL"} ${view} ${w}x${h}: sideways ${over.scroll} px${over.wide.length ? `, past the edge: ${over.wide.join(", ")}` : ""}`);
  }
  await s.send("Emulation.clearDeviceMetricsOverride");
  return lines;
}
