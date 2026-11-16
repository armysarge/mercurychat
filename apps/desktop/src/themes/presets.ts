// Themes are token sets (docs/04 §1): every preset sets every colour token and
// components use only tokens. This file and tokens.css are the only places a
// colour literal may appear (scripts/check-colours.mjs enforces it).

/** Every colour token a theme sets (CSS custom property `--<name>`). */
export const TOKEN_KEYS = [
  "canvas", "surface", "raised",
  "text", "text-2",
  "accent", "accent-text",
  "success", "warning", "danger",
  "border", "border-strong", "focus",
  "bubble-in", "bubble-out",
  "badge-beacon", "badge-cq", "badge-session",
  "waterfall-low", "waterfall-high",
  "freq-display",
  "scrim", "shadow",
] as const;
export type TokenKey = (typeof TOKEN_KEYS)[number];
export type Tokens = Record<TokenKey, string>;

/** Tokens that are translucent overlays: 8-digit hex is allowed there only, and they are not contrast-checked. */
export const ALPHA_TOKENS: readonly TokenKey[] = ["scrim", "shadow"];

export type Scheme = "dark" | "light";

export interface Theme {
  id: string;
  scheme: Scheme;
  /** Text is held to WCAG AAA (7:1) rather than AA. */
  aaa?: boolean;
  tokens: Tokens;
}

export const midnight: Theme = {
  id: "midnight",
  scheme: "dark",
  tokens: {
    canvas: "#0b1018", surface: "#141d29", raised: "#1b2838",
    text: "#e8eff8", "text-2": "#a7b5c8",
    accent: "#75d8e8", "accent-text": "#0b1018",
    success: "#88dead", warning: "#ffd186", danger: "#ff9b9b",
    border: "#263446", "border-strong": "#64788f", focus: "#75d8e8",
    "bubble-in": "#1b2838", "bubble-out": "#294454",
    "badge-beacon": "#f0b27a", "badge-cq": "#8ec5ff", "badge-session": "#c3a6ff",
    "waterfall-low": "#0e2a47", "waterfall-high": "#ffe27a",
    "freq-display": "#a8ecf6",
    scrim: "#00000059", shadow: "#00000059",
  },
};

export const daylight: Theme = {
  id: "daylight",
  scheme: "light",
  tokens: {
    canvas: "#eef2f7", surface: "#ffffff", raised: "#f5f7fa",
    text: "#162235", "text-2": "#536279",
    accent: "#126e80", "accent-text": "#ffffff",
    success: "#17683b", warning: "#805000", danger: "#a82435",
    border: "#d5dde8", "border-strong": "#74839a", focus: "#126e80",
    "bubble-in": "#ffffff", "bubble-out": "#d1e1e6",
    "badge-beacon": "#8f4a00", "badge-cq": "#1b5fa8", "badge-session": "#6b3fb0",
    "waterfall-low": "#dbe8f5", "waterfall-high": "#0b3a6b",
    "freq-display": "#0b3a47",
    scrim: "#16223540", shadow: "#16223526",
  },
};

/** Red on black, for night operating: no blue or green light, so dark-adapted vision is kept. Status is told apart by icon and word, not hue. */
export const nightRed: Theme = {
  id: "night-red",
  scheme: "dark",
  tokens: {
    canvas: "#060000", surface: "#110000", raised: "#1b0202",
    text: "#ff7568", "text-2": "#e0584c",
    accent: "#ff5a4d", "accent-text": "#060000",
    success: "#ff9084", warning: "#ff6a5c", danger: "#ff5546",
    border: "#3a0a0a", "border-strong": "#a8332b", focus: "#ff5a4d",
    "bubble-in": "#1b0202", "bubble-out": "#3a0808",
    "badge-beacon": "#ff9084", "badge-cq": "#ff6a5c", "badge-session": "#e0584c",
    "waterfall-low": "#1b0202", "waterfall-high": "#ff4a3a",
    "freq-display": "#ff4a3a",
    scrim: "#000000a0", shadow: "#000000a0",
  },
};

export const highContrastDark: Theme = {
  id: "high-contrast-dark",
  scheme: "dark",
  aaa: true,
  tokens: {
    canvas: "#000000", surface: "#000000", raised: "#0d0d0d",
    text: "#ffffff", "text-2": "#e0e0e0",
    accent: "#5cd6ff", "accent-text": "#000000",
    success: "#6dff9a", warning: "#ffd84a", danger: "#ff8a8a",
    border: "#6e6e6e", "border-strong": "#ffffff", focus: "#ffff66",
    "bubble-in": "#1a1a1a", "bubble-out": "#00334a",
    "badge-beacon": "#ffb870", "badge-cq": "#8cc8ff", "badge-session": "#d0b8ff",
    "waterfall-low": "#000000", "waterfall-high": "#ffff66",
    "freq-display": "#8ee8ff",
    scrim: "#000000b3", shadow: "#000000b3",
  },
};

export const highContrastLight: Theme = {
  id: "high-contrast-light",
  scheme: "light",
  aaa: true,
  tokens: {
    canvas: "#ffffff", surface: "#ffffff", raised: "#f2f2f2",
    text: "#000000", "text-2": "#262626",
    accent: "#004a99", "accent-text": "#ffffff",
    success: "#005a1f", warning: "#6b3a00", danger: "#8f0014",
    border: "#767676", "border-strong": "#000000", focus: "#0000cc",
    "bubble-in": "#f2f2f2", "bubble-out": "#cfe3ff",
    "badge-beacon": "#7a3d00", "badge-cq": "#00428a", "badge-session": "#55228f",
    "waterfall-low": "#ffffff", "waterfall-high": "#000000",
    "freq-display": "#002b5c",
    scrim: "#00000080", shadow: "#00000040",
  },
};

export const ocean: Theme = {
  id: "ocean",
  scheme: "dark",
  tokens: {
    canvas: "#071a2b", surface: "#0c2740", raised: "#12324f",
    text: "#e6f4ff", "text-2": "#9fc3dd",
    accent: "#2fd6c4", "accent-text": "#04202a",
    success: "#7be0a0", warning: "#f6cf6b", danger: "#ff9a9a",
    border: "#1d4468", "border-strong": "#5a8fb5", focus: "#2fd6c4",
    "bubble-in": "#12324f", "bubble-out": "#0e4c58",
    "badge-beacon": "#f0b27a", "badge-cq": "#8ec5ff", "badge-session": "#c3a6ff",
    "waterfall-low": "#06223a", "waterfall-high": "#7ff0e2",
    "freq-display": "#7ff0e2",
    scrim: "#00101e80", shadow: "#00101e80",
  },
};

export const slate: Theme = {
  id: "slate",
  scheme: "dark",
  tokens: {
    canvas: "#16181b", surface: "#1f2226", raised: "#292d32",
    text: "#e6e8eb", "text-2": "#a3a9b1",
    accent: "#b8c4d6", "accent-text": "#16181b",
    success: "#9bd4ae", warning: "#e3c88a", danger: "#e8a0a0",
    border: "#33383e", "border-strong": "#737a84", focus: "#b8c4d6",
    "bubble-in": "#292d32", "bubble-out": "#343b45",
    "badge-beacon": "#dcb48c", "badge-cq": "#a3bfe0", "badge-session": "#bfb0e0",
    "waterfall-low": "#1a1d21", "waterfall-high": "#e6e8eb",
    "freq-display": "#dfe6f0",
    scrim: "#00000066", shadow: "#00000066",
  },
};

/** A warm monochrome nod to classic terminals; status is carried by icon and word. */
export const amberTerminal: Theme = {
  id: "amber-terminal",
  scheme: "dark",
  tokens: {
    canvas: "#0c0800", surface: "#140d00", raised: "#1d1400",
    text: "#ffb000", "text-2": "#cc8c00",
    accent: "#ffc933", "accent-text": "#0c0800",
    success: "#ffd36b", warning: "#ff9d2e", danger: "#ff8a5c",
    border: "#3b2a00", "border-strong": "#8a6200", focus: "#ffc933",
    "bubble-in": "#1d1400", "bubble-out": "#3a2800",
    "badge-beacon": "#ffd36b", "badge-cq": "#ff9d2e", "badge-session": "#cc8c00",
    "waterfall-low": "#1d1400", "waterfall-high": "#ffc933",
    "freq-display": "#ffb000",
    scrim: "#00000080", shadow: "#00000080",
  },
};

/** The concrete presets, in the order the picker lists them (after System). */
export const PRESETS: readonly Theme[] = [midnight, daylight, nightRed, highContrastDark, highContrastLight, ocean, slate, amberTerminal];

/** "System" follows the OS and is not a token set of its own. */
export const SYSTEM_ID = "system";

export function presetById(id: string): Theme | undefined {
  return PRESETS.find((p) => p.id === id);
}

/** The concrete preset behind a theme id: System resolves by the OS setting. Unknown ids give Midnight. */
export function resolvePreset(id: string, prefersDark: boolean): Theme {
  if (id === SYSTEM_ID) return prefersDark ? midnight : daylight;
  return presetById(id) ?? midnight;
}
