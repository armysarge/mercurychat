// The built-in modem of this build (M5): its Mercury and Hamlib versions,
// shown next to every mention of them (owner, 23 Nov), and whether its
// shipped files are as built. Loaded once at start.
import { ref } from "vue";
import { call } from "./api";
import type { BuiltInView } from "./bindings/BuiltInView";
import { info } from "./diag";

export const builtin = ref<BuiltInView | null>(null);

export async function loadBuiltin(): Promise<void> {
  try {
    const r = await call({ type: "builtInInfo" });
    if (r.type === "builtIn") builtin.value = { available: r.available, mercury: r.mercury, hamlib: r.hamlib, integrity: r.integrity };
  } catch {
    builtin.value = null;
  }
}

/** The sound systems a built-in Mercury is offered, preferred first (M5 §5.5). */
export function builtinSystems(): string[] {
  return info.value?.os === "windows" ? ["wasapi", "dsound"] : ["pulse", "alsa"];
}

/** A Settings tab to open next time Settings is shown (the strip's "Choose a sound card"). */
export const settingsTabRequest = ref<string | null>(null);
