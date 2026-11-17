// The window's appearance state: loaded from the shell before the first
// paint, applied the instant it changes, and saved (debounced) to
// appearance.json in the data root, where the splash can read it too.
import { invoke } from "@tauri-apps/api/core";
import { computed, reactive, watch } from "vue";
import {
  DEFAULT_SAVED, applySaved, clampFontScale, parseSaved, serialiseSaved, themeFor, type CustomTheme, type Density, type Saved,
} from "./apply.ts";
import { midnight, presetById, type Theme } from "./presets.ts";

export const baseFor = (id: string): Theme => presetById(id) ?? midnight;

const dark = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;
const prefersDark = () => dark?.matches ?? true;

export const appearance = reactive<Saved & { loaded: boolean; saveError: string }>({ ...DEFAULT_SAVED, custom: [], loaded: false, saveError: "" });

/** The theme in use, resolved (System follows the OS). */
export const activeTheme = computed(() => themeFor(appearance, prefersDark()));
const osScheme = reactive({ dark: prefersDark() });
dark?.addEventListener("change", () => (osScheme.dark = prefersDark()));

const snapshotOf = (): Saved => ({ theme: appearance.theme, custom: appearance.custom, density: appearance.density, fontScale: appearance.fontScale });

let timer: ReturnType<typeof setTimeout> | undefined;
function persist() {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    try {
      await invoke("appearance_save", { json: serialiseSaved(snapshotOf(), prefersDark()) });
      appearance.saveError = "";
    } catch (e) {
      appearance.saveError = String(e);
    }
  }, 250);
}

/** Apply now, and save. */
function changed() {
  applySaved(document.documentElement, snapshotOf(), prefersDark());
  persist();
}

/** Load the saved appearance and apply it; the app mounts after this (a missing file means defaults). */
export async function initAppearance() {
  let text: string | null = null;
  try {
    text = await invoke<string | null>("appearance_load");
  } catch {
    // Not running in the shell (or no file yet): defaults.
  }
  Object.assign(appearance, parseSaved(text, baseFor), { loaded: true });
  applySaved(document.documentElement, snapshotOf(), prefersDark());
  // "System" tracks the OS setting while the window is open; the saved colours follow it.
  watch(() => osScheme.dark, () => appearance.theme === "system" && changed());
  if (text === null || appearance.theme === "system") persist();
}

export function setTheme(id: string) {
  appearance.theme = id;
  changed();
}
export function setDensity(d: Density) {
  appearance.density = d;
  changed();
}
export function setFontScale(percent: number) {
  appearance.fontScale = clampFontScale(percent);
  changed();
}
export const MAX_CUSTOM_THEMES = 24;
/** Save a custom theme (a new one, or `id` replaced) and switch to it. */
export function saveCustom(theme: Omit<CustomTheme, "id">, id?: string): string | null {
  const existing = id ? appearance.custom.findIndex((c) => c.id === id) : -1;
  if (existing < 0 && appearance.custom.length >= MAX_CUSTOM_THEMES) return null;
  const finalId = existing >= 0 ? appearance.custom[existing].id : `c-${Date.now().toString(36)}${Math.floor(Math.random() * 1296).toString(36)}`;
  const entry = { id: finalId, ...theme };
  if (existing >= 0) appearance.custom.splice(existing, 1, entry);
  else appearance.custom.push(entry);
  appearance.theme = finalId;
  changed();
  return finalId;
}
export function deleteCustom(id: string) {
  const i = appearance.custom.findIndex((c) => c.id === id);
  if (i < 0) return;
  appearance.custom.splice(i, 1);
  if (appearance.theme === id) appearance.theme = "system";
  changed();
}
/** Show a draft theme without saving it (the editor's live preview); `undo` restores the saved one. */
export function previewTheme(theme: { scheme: "dark" | "light"; tokens: Theme["tokens"] }) {
  applySaved(document.documentElement, { ...snapshotOf(), theme: "__preview", custom: [{ id: "__preview", name: "", base: "midnight", ...theme }] }, prefersDark());
}
export function undoPreview() {
  applySaved(document.documentElement, snapshotOf(), prefersDark());
}
