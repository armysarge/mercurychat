// The start-up splash: a second, tiny page of the same app. It uses the same
// translations; the language last chosen in the main window is remembered
// (the core's setting is not readable yet while the splash is up).
import { createApp } from "vue";
import { applyLanguage, i18n, storedLanguage } from "../i18n";
import "../tokens.css";
import "../ui.css";
import { invoke } from "@tauri-apps/api/core";
import { applySaved, parseSaved } from "../themes/apply.ts";
import { midnight, presetById } from "../themes/presets.ts";
import Splash from "./Splash.vue";

applyLanguage(storedLanguage());
// The saved theme, read from the data root by the shell (the core is not open yet).
async function paint() {
  try {
    const text = await invoke<string | null>("appearance_load");
    const dark = matchMedia("(prefers-color-scheme: dark)").matches;
    applySaved(document.documentElement, parseSaved(text, (id) => presetById(id) ?? midnight), dark);
  } catch {
    // No saved theme (or not in the shell): the stylesheet defaults stand.
  }
}
void paint().finally(() => createApp(Splash).use(i18n).mount("#app"));
