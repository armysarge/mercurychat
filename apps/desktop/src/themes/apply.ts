// Applying a theme to the document, and importing/exporting custom themes.
// Only validated hex colours ever reach `setProperty`: nothing from an
// imported file or a saved file is put into CSS as a raw string.
import { ALPHA_TOKENS, TOKEN_KEYS, midnight, resolvePreset, type Scheme, type TokenKey, type Tokens } from "./presets.ts";

/** The slice of an element's style that is written to (document.documentElement in the app, a fake in tests). */
export interface RootLike {
  style: { setProperty(name: string, value: string): void; removeProperty(name: string): void };
  setAttribute(name: string, value: string): void;
}

const HEX_OPAQUE = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;
const HEX_ALPHA = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;

/** A valid value for the token: a hex colour, with an alpha channel only on the overlay tokens. */
export function isColour(key: string, value: unknown): value is string {
  if (typeof value !== "string") return false;
  return (ALPHA_TOKENS as readonly string[]).includes(key) ? HEX_ALPHA.test(value) : HEX_OPAQUE.test(value);
}

/** Set a theme's tokens on the root. A value that is not a valid colour is replaced by the same token of `base` (Midnight by default). */
export function applyTheme(root: RootLike, theme: { scheme: Scheme; tokens: Partial<Record<TokenKey, string>> }, base: Tokens = midnight.tokens) {
  for (const key of TOKEN_KEYS) {
    const value = theme.tokens[key];
    if (isColour(key, value)) root.style.setProperty(`--${key}`, value);
    else root.style.setProperty(`--${key}`, base[key]);
  }
  root.style.setProperty("color-scheme", theme.scheme === "light" ? "light" : "dark");
}

export type Density = "normal" | "compact";
export const FONT_SCALE_MIN = 90;
export const FONT_SCALE_MAX = 130;

export function normaliseDensity(value: unknown): Density {
  return value === "compact" ? "compact" : "normal";
}
export function clampFontScale(percent: number): number {
  if (!Number.isFinite(percent)) return 100;
  return Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, Math.round(percent)));
}
export function applyDensity(root: RootLike, density: unknown) {
  root.setAttribute("data-density", normaliseDensity(density));
}
export function applyFontScale(root: RootLike, percent: number) {
  root.style.setProperty("--font-scale", String(clampFontScale(percent) / 100));
}

// ---- custom themes: export and import

export interface CustomTheme {
  id: string;
  name: string;
  /** The preset it started from (missing tokens on import come from it). */
  base: string;
  scheme: Scheme;
  tokens: Tokens;
}

export const THEME_FILE_APP = "mercurychat-theme";
export const THEME_FILE_VERSION = 1;
/** An imported file larger than this is refused before it is parsed. */
export const MAX_IMPORT_BYTES = 16 * 1024;
export const MAX_NAME = 40;

export type ImportFailure = { ok: false; reason: "tooLarge" | "notJson" | "notTheme" | "version" | "badColour"; detail?: string };
export type ImportResult = { ok: true; theme: Omit<CustomTheme, "id"> } | ImportFailure;

export function cleanName(value: unknown): string {
  // eslint-disable-next-line no-control-regex
  const s = typeof value === "string" ? value.replace(/[\u0000-\u001f\u007f]/g, " ").replace(/\s+/g, " ").trim() : "";
  return s.slice(0, MAX_NAME).trim();
}

/** The file written by Export: versioned, with the full token set. */
export function exportTheme(theme: Pick<CustomTheme, "name" | "base" | "scheme" | "tokens">): string {
  const tokens: Record<string, string> = {};
  for (const k of TOKEN_KEYS) tokens[k] = theme.tokens[k];
  return JSON.stringify({ app: THEME_FILE_APP, version: THEME_FILE_VERSION, name: theme.name, base: theme.base, scheme: theme.scheme, tokens }, null, 2);
}

/**
 * Validate an imported theme file. Unknown keys are ignored; a missing token
 * is filled from the base preset (`baseFor(id)`; an unknown base gives
 * Midnight's); any present token that is not a hex colour rejects the file.
 */
export function parseThemeImport(
  text: string,
  baseFor: (id: string) => { id: string; scheme: Scheme; tokens: Tokens },
): ImportResult {
  if (new TextEncoder().encode(text).length > MAX_IMPORT_BYTES) return { ok: false, reason: "tooLarge" };
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    return { ok: false, reason: "notJson" };
  }
  if (typeof data !== "object" || data === null || Array.isArray(data)) return { ok: false, reason: "notTheme" };
  const d = data as Record<string, unknown>;
  if (d.app !== THEME_FILE_APP || typeof d.tokens !== "object" || d.tokens === null || Array.isArray(d.tokens)) {
    return { ok: false, reason: "notTheme" };
  }
  if (d.version !== THEME_FILE_VERSION) return { ok: false, reason: "version" };
  const base = baseFor(typeof d.base === "string" ? d.base : "");
  const given = d.tokens as Record<string, unknown>;
  const tokens = { ...base.tokens };
  for (const key of TOKEN_KEYS) {
    if (!Object.prototype.hasOwnProperty.call(given, key)) continue;
    if (!isColour(key, given[key])) return { ok: false, reason: "badColour", detail: key };
    tokens[key] = given[key];
  }
  const scheme: Scheme = d.scheme === "light" || d.scheme === "dark" ? d.scheme : base.scheme;
  return { ok: true, theme: { name: cleanName(d.name) || "Imported theme", base: base.id, scheme, tokens } };
}

// ---- the saved appearance (appearance.json via the shell)

/** What is saved: which theme, the custom ones, density and font scale. */
export interface Saved {
  theme: string;
  custom: CustomTheme[];
  density: Density;
  fontScale: number;
}
export const DEFAULT_SAVED: Saved = { theme: "system", custom: [], density: "normal", fontScale: 100 };

const SLUG = /^[a-z0-9-]{1,48}$/;
const MAX_CUSTOM = 24;

/** Read the shell's saved JSON defensively: anything wrong falls back to defaults, value by value. */
export function parseSaved(text: string | null, baseFor: (id: string) => { id: string; scheme: Scheme; tokens: Tokens }): Saved {
  let data: unknown;
  try {
    data = text === null ? null : JSON.parse(text);
  } catch {
    data = null;
  }
  if (typeof data !== "object" || data === null || Array.isArray(data)) return { ...DEFAULT_SAVED, custom: [] };
  const d = data as Record<string, unknown>;
  const custom: CustomTheme[] = [];
  for (const c of Array.isArray(d.custom) ? d.custom.slice(0, MAX_CUSTOM) : []) {
    if (typeof c !== "object" || c === null) continue;
    const r = c as Record<string, unknown>;
    if (typeof r.id !== "string" || !SLUG.test(r.id)) continue;
    const base = baseFor(typeof r.base === "string" ? r.base : "");
    const given = typeof r.tokens === "object" && r.tokens !== null ? (r.tokens as Record<string, unknown>) : {};
    const tokens = { ...base.tokens };
    for (const key of TOKEN_KEYS) if (isColour(key, given[key])) tokens[key] = given[key];
    custom.push({ id: r.id, name: cleanName(r.name) || r.id, base: base.id, scheme: r.scheme === "light" || r.scheme === "dark" ? r.scheme : base.scheme, tokens });
  }
  const theme = typeof d.theme === "string" && SLUG.test(d.theme) ? d.theme : DEFAULT_SAVED.theme;
  return {
    theme,
    custom,
    density: normaliseDensity(d.density),
    fontScale: typeof d.fontScale === "number" ? clampFontScale(d.fontScale) : 100,
  };
}

/** The theme to show for a saved choice: a custom theme, a preset, or System by the OS setting. Unknown ids give Midnight (or Daylight on a light OS). */
export function themeFor(saved: Pick<Saved, "theme" | "custom">, prefersDark: boolean): { id: string; scheme: Scheme; aaa?: boolean; tokens: Tokens } {
  const own = saved.custom.find((c) => c.id === saved.theme);
  return own ?? resolvePreset(saved.theme, prefersDark);
}

/** The JSON the shell stores: the active theme's resolved colours go along for the splash. */
export function serialiseSaved(saved: Saved, prefersDark: boolean): string {
  const active = themeFor(saved, prefersDark);
  return JSON.stringify({
    version: 1,
    theme: saved.theme,
    scheme: active.scheme,
    tokens: active.tokens,
    density: saved.density,
    fontScale: clampFontScale(saved.fontScale),
    custom: saved.custom.map(({ id, name, base, scheme, tokens }) => ({ id, name, base, scheme, tokens })),
  });
}

/** Apply a saved appearance to the document in one go. */
export function applySaved(root: RootLike, saved: Saved, prefersDark: boolean) {
  applyTheme(root, themeFor(saved, prefersDark));
  applyDensity(root, saved.density);
  applyFontScale(root, saved.fontScale);
}
