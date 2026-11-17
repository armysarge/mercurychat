// WCAG 2.x contrast: relative luminance with sRGB linearisation, the checks a
// theme must pass, and the deterministic lightness "auto-fix".
import { ALPHA_TOKENS, TOKEN_KEYS, type Theme, type TokenKey, type Tokens } from "./presets.ts";

export interface Rgb { r: number; g: number; b: number }

/** #rgb, #rrggbb or #rrggbbaa (alpha ignored) to 0-255 channels. */
export function hexToRgb(hex: string): Rgb {
  let h = hex.replace("#", "");
  if (h.length === 3) h = [...h].map((c) => c + c).join("");
  return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16) };
}
export function rgbToHex({ r, g, b }: Rgb): string {
  return "#" + [r, g, b].map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, "0")).join("");
}

const lin = (c: number) => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};
export function relativeLuminance(hex: string): number {
  const { r, g, b } = hexToRgb(hex);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}
export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

export interface Rule { fg: TokenKey; bg: TokenKey; min: number; kind: "text" | "ui" | "decor" }
export interface Failure extends Rule { ratio: number }

const SURFACES: TokenKey[] = ["canvas", "surface", "raised", "bubble-in", "bubble-out"];
const BASE3: TokenKey[] = ["canvas", "surface", "raised"];
/** Lowest ratio the decorative divider needs to stay visible (not a WCAG requirement). */
export const DECOR_MIN = 1.1;

/** What a theme must satisfy: text 4.5:1 (7:1 for AAA themes), UI parts 3:1. */
export function rulesFor(theme: Pick<Theme, "aaa">): Rule[] {
  const text = theme.aaa ? 7 : 4.5;
  const rules: Rule[] = [];
  for (const fg of ["text", "text-2"] as TokenKey[]) for (const bg of SURFACES) rules.push({ fg, bg, min: text, kind: "text" });
  for (const fg of ["accent", "success", "warning", "danger", "badge-beacon", "badge-cq", "badge-session"] as TokenKey[]) {
    for (const bg of BASE3) rules.push({ fg, bg, min: text, kind: "text" });
  }
  for (const bg of ["surface", "raised"] as TokenKey[]) rules.push({ fg: "freq-display", bg, min: text, kind: "text" });
  rules.push({ fg: "accent-text", bg: "accent", min: text, kind: "text" });
  for (const fg of ["border-strong", "focus"] as TokenKey[]) for (const bg of BASE3) rules.push({ fg, bg, min: 3, kind: "ui" });
  rules.push({ fg: "waterfall-high", bg: "waterfall-low", min: 3, kind: "ui" });
  for (const bg of BASE3) rules.push({ fg: "border", bg, min: DECOR_MIN, kind: "decor" });
  return rules;
}

export function checkTheme(theme: Pick<Theme, "aaa" | "tokens">): Failure[] {
  const out: Failure[] = [];
  for (const rule of rulesFor(theme)) {
    const ratio = contrastRatio(theme.tokens[rule.fg], theme.tokens[rule.bg]);
    if (ratio < rule.min) out.push({ ...rule, ratio });
  }
  return out;
}

// ---- auto-fix

function hexToHsl(hex: string): [number, number, number] {
  const { r, g, b } = hexToRgb(hex);
  const [R, G, B] = [r / 255, g / 255, b / 255];
  const max = Math.max(R, G, B);
  const min = Math.min(R, G, B);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  const h = max === R ? (G - B) / d + (G < B ? 6 : 0) : max === G ? (B - R) / d + 2 : (R - G) / d + 4;
  return [h * 60, s, l];
}
function hslToHex(h: number, s: number, l: number): string {
  const f = (n: number) => {
    const k = (n + h / 30) % 12;
    return l - s * Math.min(l, 1 - l) * Math.max(-1, Math.min(k - 3, 9 - k, 1));
  };
  return rgbToHex({ r: f(0) * 255, g: f(8) * 255, b: f(4) * 255 });
}

const STEP = 0.005;
const MAX_STEPS = 200;

/** Move a colour's HSL lightness one way until `ok`, at most MAX_STEPS steps; null when it never gets there. */
function nudge(hex: string, dir: 1 | -1, ok: (c: string) => boolean): string | null {
  const [h, s, l0] = hexToHsl(hex);
  for (let i = 1; i <= MAX_STEPS; i++) {
    const l = l0 + dir * STEP * i;
    if (l > 1 || l < 0) return null;
    const c = hslToHex(h, s, l);
    if (ok(c)) return c;
  }
  return null;
}

export interface FixResult { tokens: Tokens; changed: TokenKey[]; passes: boolean }

/**
 * Nudge the lightness of the foreground tokens that fail until the theme
 * passes; surfaces stay as chosen. Deterministic: a fixed step, a fixed order
 * (accent-text last, since it depends on the accent), the direction away from
 * the surfaces a colour sits on, and at most 4 rounds.
 */
export function autoFix(theme: Pick<Theme, "aaa" | "tokens">): FixResult {
  const tokens: Tokens = { ...theme.tokens };
  const changed = new Set<TokenKey>();
  const order = TOKEN_KEYS.filter((k) => k !== "accent-text" && !ALPHA_TOKENS.includes(k));
  order.push("accent-text");
  for (let round = 0; round < 4; round++) {
    if (checkTheme({ aaa: theme.aaa, tokens }).length === 0) break;
    for (const key of order) {
      const rules = rulesFor(theme).filter((r) => r.fg === key);
      const bad = (t: Tokens) => rules.some((r) => contrastRatio(t[r.fg], t[r.bg]) < r.min);
      if (!rules.length || !bad(tokens)) continue;
      const bgs = rules.map((r) => relativeLuminance(tokens[r.bg]));
      const mean = bgs.reduce((a, b) => a + b, 0) / bgs.length;
      const first: 1 | -1 = mean < 0.18 ? 1 : -1;
      const ok = (c: string) => !bad({ ...tokens, [key]: c });
      const fixed = nudge(tokens[key], first, ok) ?? nudge(tokens[key], first === 1 ? -1 : 1, ok);
      if (fixed) {
        tokens[key] = fixed;
        changed.add(key);
      }
    }
  }
  return { tokens, changed: [...changed], passes: checkTheme({ aaa: theme.aaa, tokens }).length === 0 };
}

/** Share (0-1) of a colour's relative luminance that comes from its green and blue channels. */
export function greenBlueShare(hex: string): number {
  const { r, g, b } = hexToRgb(hex);
  const total = relativeLuminance(hex);
  return total === 0 ? 0 : (0.7152 * lin(g) + 0.0722 * lin(b)) / total;
}
