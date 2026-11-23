// The first-run wizard's progress as the core keeps it (M5 §6): loaded once
// at start; every step change is saved there first, so a restart resumes it.
import { ref } from "vue";
import { call } from "./api";
import type { WizardState } from "./bindings/WizardState";

/** `null` until loaded, or when it could not be read (the plain setup screen then). */
export const wizard = ref<WizardState | null>(null);
export const wizardLoaded = ref(false);

export async function loadWizard(): Promise<void> {
  try {
    const r = await call({ type: "getWizard" });
    if (r.type === "wizard") {
      const { type: _, ...w } = r;
      wizard.value = w;
    }
  } catch {
    wizard.value = null;
  } finally {
    wizardLoaded.value = true;
  }
}

/** Keep `state` in the core; shown once it is kept (a refused save leaves the step as it was). */
export async function saveWizard(state: WizardState): Promise<void> {
  const r = await call({ type: "setWizard", state });
  if (r.type === "wizard") {
    const { type: _, ...w } = r;
    wizard.value = w;
  }
}

/** Settings → "Run the setup again": from the start, the skipped list cleared. */
export const restartWizard = (): Promise<void> =>
  saveWizard({ step: "welcome", draftProfileId: null, done: false, skipped: [], station: null });
