// M5 Task 13: the accessibility measurements the a11y check runs on each view,
// in the real window over CDP.
//   tabWalk   real Tab key presses from the top: every visible control is
//             reached, in DOM order, and focus never stays put (no trap)
//   unnamed   controls Chromium's own accessibility tree gives no name
//   lowContrast  visible text under 4.5:1 (3:1 when large), from computed styles
import { sleep } from "./cdp.mjs";

/** Mark every control Tab should reach, in DOM order; returns how many. */
const MARK = `(() => {
  document.querySelectorAll('[data-a11y]').forEach(e => e.removeAttribute('data-a11y'));
  const shown = (e) => {
    if (!e.getClientRects().length) return false;
    // Inside a closed <details>: laid out, but content-visibility hides it.
    if (e.checkVisibility && !e.checkVisibility({ visibilityProperty: true })) return false;
    for (let p = e; p; p = p.parentElement) {
      const cs = getComputedStyle(p);
      if (cs.visibility === 'hidden' || cs.display === 'none') return false;
      if (p.inert) return false;
    }
    return true;
  };
  const sel = 'a[href], button, input, select, textarea, summary, [tabindex], [contenteditable=true]';
  const radiosDone = new Set();
  const list = [...document.querySelectorAll(sel)].filter(e => {
    if (e.tabIndex < 0 || e.matches(':disabled') || !shown(e)) return false;
    if (e.type === 'hidden') return false;
    if (e.type === 'radio' && e.name) {
      // One stop per radio group: the checked one, or the first.
      const group = [...document.querySelectorAll('input[type=radio][name="' + CSS.escape(e.name) + '"]')].filter(r => !r.disabled && shown(r));
      const stop = group.find(r => r.checked) ?? group[0];
      if (e !== stop || radiosDone.has(e.name)) return false;
      radiosDone.add(e.name);
    }
    return true;
  });
  const positive = [...document.querySelectorAll('[tabindex]')].filter(e => e.tabIndex > 0).length;
  list.forEach((e, i) => e.setAttribute('data-a11y', String(i)));
  return { n: list.length, positive };
})()`;

const describe = `((e) => !e ? 'nothing' : e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') + (typeof e.className === 'string' && e.className ? '.' + e.className.trim().split(/\\s+/).join('.') : '') + (e.getAttribute('aria-label') ? '[' + e.getAttribute('aria-label') + ']' : ''))`;

async function pressTab(s, shift = false) {
  const base = { key: "Tab", code: "Tab", windowsVirtualKeyCode: 9, nativeVirtualKeyCode: 9, modifiers: shift ? 8 : 0 };
  await s.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
  await s.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
}

/** Tab through the view. Returns [] or the findings in words. */
export async function tabWalk(s) {
  const found = [];
  const { n, positive } = await s.eval(MARK);
  if (positive) found.push(`${positive} element(s) with a positive tabindex (order not DOM order)`);
  // Start at the top of the page: a focus point before everything (blur alone
  // leaves Chromium's sequential-focus start where focus last was).
  await s.eval(`(() => {
    const a = document.createElement('span');
    a.id = 'a11y-start'; a.tabIndex = -1; a.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px';
    document.body.prepend(a); a.focus(); window.scrollTo(0, 0); return true;
  })()`);
  const seen = [];
  let last = null;
  let stuck = 0;
  for (let i = 0; i < n * 2 + 10; i++) {
    await pressTab(s);
    await sleep(60);
    const at = await s.eval(`(() => {
      const e = document.activeElement;
      if (e) e.__a11yId ??= (window.__a11yN = (window.__a11yN ?? 0) + 1);
      const tag = e?.tagName?.toLowerCase();
      const control = !!e && e.tabIndex >= 0 && e.getClientRects().length > 0 && tag !== 'body' && tag !== 'html';
      return { idx: e?.getAttribute?.('data-a11y') ?? null, id: e?.__a11yId ?? 0, control, what: ${describe}(e) };
    })()`);
    if (at.id === last) {
      if (++stuck >= 3) {
        found.push(`focus is trapped on ${at.what}`);
        break;
      }
    } else stuck = 0;
    last = at.id;
    if (at.idx === null) {
      if (at.what === "body" || at.what === "nothing" || at.what === "html") {
        if (seen.length >= n) break; // past the last control: out of the page
        continue;
      }
      // A control that appeared after the list was taken (the dial's digits once the rig is up) is fine.
      if (!at.control) found.push(`Tab reached ${at.what}, which is not a visible control`);
      continue;
    }
    const idx = Number(at.idx);
    if (seen.length && idx < seen[seen.length - 1]) break; // wrapped round to the top
    seen.push(idx);
  }
  await s.eval("document.getElementById('a11y-start')?.remove()");
  const order = seen.every((v, i) => i === 0 || v > seen[i - 1]);
  if (!order) found.push(`Tab order is not DOM order: ${seen.slice(0, 20).join(",")}`);
  const missed = [];
  for (let i = 0; i < n; i++) if (!seen.includes(i)) missed.push(i);
  if (missed.length) {
    const names = await s.eval(`[${missed.slice(0, 6).join(",")}].map(i => ${describe}(document.querySelector('[data-a11y="' + i + '"]')))`);
    found.push(`Tab never reached ${missed.length} of ${n}: ${names.join(", ")}`);
  }
  return { n, reached: seen.length, found };
}

const ROLES = new Set(["button", "textbox", "searchbox", "combobox", "checkbox", "switch", "slider", "spinbutton", "radio", "listbox", "menuitem", "tab", "link"]);

/** Controls the accessibility tree gives no name (Chromium's own accname). */
export async function unnamed(s) {
  await s.send("Accessibility.enable");
  const { nodes } = await s.send("Accessibility.getFullAXTree");
  const bad = nodes.filter((x) => !x.ignored && ROLES.has(x.role?.value) && !(x.name?.value ?? "").trim() && x.backendDOMNodeId);
  const out = [];
  for (const x of bad.slice(0, 12)) {
    try {
      const { object } = await s.send("DOM.resolveNode", { backendNodeId: x.backendDOMNodeId });
      const r = await s.send("Runtime.callFunctionOn", { objectId: object.objectId, functionDeclaration: `function () { return ${describe}(this) + (this.getClientRects().length ? '' : ' (not shown)'); }`, returnByValue: true });
      if (!/\(not shown\)$/.test(r.result.value)) out.push(`${x.role.value} ${r.result.value}`);
    } catch {
      out.push(`${x.role.value} #${x.backendDOMNodeId}`);
    }
  }
  return out;
}

/** Visible text below WCAG AA contrast against what is painted behind it. */
export const lowContrast = (s) =>
  s.eval(`(() => {
  const parse = (c) => {
    let m = c.match(/rgba?\\(([^)]+)\\)/);
    if (m) { const p = m[1].split(/[ ,/]+/).filter(Boolean).map(Number); return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1]; }
    m = c.match(/color\\(srgb ([^)]+)\\)/);
    if (m) { const p = m[1].split(/[ /]+/).filter(Boolean).map(Number); return [p[0] * 255, p[1] * 255, p[2] * 255, p.length > 3 ? p[3] : 1]; }
    return null;
  };
  const over = (top, under) => { const a = top[3]; return [0, 1, 2].map(i => top[i] * a + under[i] * (1 - a)).concat(1); };
  const lum = (c) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]); };
  const bgOf = (e) => {
    const stack = [];
    for (let p = e; p; p = p.parentElement) { const c = parse(getComputedStyle(p).backgroundColor); if (c && c[3] > 0) { stack.push(c); if (c[3] >= 1) break; } }
    let bg = parse(getComputedStyle(document.body).backgroundColor) ?? [0, 0, 0, 1];
    if (bg[3] < 1) bg = over(bg, [255, 255, 255, 1]);
    for (let i = stack.length - 1; i >= 0; i--) bg = over(stack[i], bg);
    return bg;
  };
  const hasImage = (e) => { for (let p = e; p; p = p.parentElement) if (getComputedStyle(p).backgroundImage !== 'none') return true; return false; };
  const out = [];
  const seen = new Set();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const t = walker.currentNode;
    if (!t.data.trim()) continue;
    const e = t.parentElement;
    if (!e || seen.has(e)) continue;
    seen.add(e);
    if (!e.getClientRects().length) continue;
    const cs = getComputedStyle(e);
    if (cs.visibility === 'hidden' || Number(cs.opacity) === 0) continue;
    // Disabled controls are exempt (WCAG 1.4.3: inactive components).
    if (e.closest('button:disabled, input:disabled, select:disabled, textarea:disabled, [aria-disabled=true], fieldset:disabled')) continue;
    if (e.closest('.sr-only')) continue;
    if (hasImage(e) && !e.closest('button, .card, .pill')) continue;
    const fg0 = parse(cs.color);
    if (!fg0) continue;
    let op = 1; for (let p = e; p; p = p.parentElement) op *= Number(getComputedStyle(p).opacity);
    const bg = bgOf(e);
    const fg = over([fg0[0], fg0[1], fg0[2], fg0[3] * op], bg);
    const L1 = lum(fg), L2 = lum(bg);
    const ratio = (Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05);
    const px = parseFloat(cs.fontSize), bold = Number(cs.fontWeight) >= 700;
    const large = px >= 24 || (bold && px >= 18.66);
    const need = large ? 3 : 4.5;
    if (ratio < need - 0.01) {
      out.push((e.tagName.toLowerCase() + (typeof e.className === 'string' && e.className ? '.' + e.className.trim().split(/\\s+/)[0] : '')) + ' "' + t.data.trim().slice(0, 30) + '" ' + ratio.toFixed(2) + ':1 (needs ' + need + ')');
    }
  }
  return out.slice(0, 12);
})()`);

/** The station panel, open, stays one row: no taller than 80 px and nothing wrapped below. */
export const panelOneRow = (s) =>
  s.eval(`(() => {
  const p = document.querySelector('section.strip');
  if (!p || getComputedStyle(p).display === 'none') return 'the panel is not open';
  const r = p.getBoundingClientRect();
  if (r.height > 80.5) return 'the panel is ' + Math.round(r.height) + ' px tall';
  const groups = [...p.querySelectorAll(':scope > .groups > .group')].filter(g => g.getClientRects().length);
  const tops = new Set(groups.map(g => Math.round(g.getBoundingClientRect().top)));
  return tops.size <= 1 ? '' : 'the panel wraps onto ' + tops.size + ' rows';
})()`);

/** The header's items (and the window controls in it) neither overlap nor leave the window. */
export const headerFits = (s) =>
  s.eval(`(() => {
  const h = document.querySelector('header');
  const items = [...h.children, ...h.querySelectorAll('.winctl button')]
    .filter(e => e.getClientRects().length && getComputedStyle(e).visibility !== 'hidden' && !e.classList.contains('grow'));
  const name = (e) => e.tagName.toLowerCase() + (typeof e.className === 'string' && e.className ? '.' + e.className.trim().split(/\s+/)[0] : '') + (e.getAttribute('aria-label') ? '[' + e.getAttribute('aria-label') + ']' : '');
  const out = [];
  const rs = items.map(e => [e, e.getBoundingClientRect()]);
  for (const [e, r] of rs) if (r.right > innerWidth + 0.5 || r.left < -0.5) out.push(name(e) + ' leaves the window');
  for (let i = 0; i < rs.length; i++) for (let j = i + 1; j < rs.length; j++) {
    const [a, ra] = rs[i], [b, rb] = rs[j];
    if (a.contains(b) || b.contains(a)) continue;
    const x = Math.min(ra.right, rb.right) - Math.max(ra.left, rb.left);
    if (x > 1 && Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top) > 1) out.push(name(a) + ' overlaps ' + name(b) + ' by ' + Math.round(x) + ' px');
  }
  return out.slice(0, 5).join('; ');
})()`);
