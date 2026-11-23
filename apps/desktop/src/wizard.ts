// The first-run wizard's steps (M5 §6): their order on each path, and Back,
// Next and Skip. Pure, so node can test it (scripts/wizard.test.mjs); the
// progress itself is kept by the core (wizardState.ts).
import type { WizardState } from "./bindings/WizardState";
import type { WizardStep } from "./bindings/WizardStep";

/** Built-in Mercury, My own Mercury, or not chosen yet. */
export type WizardPath = "builtIn" | "existing" | null;

export const STEPS = ["welcome", "station", "modem", "audio", "rig", "operate", "tour", "done"] as const satisfies readonly WizardStep[];

/** Welcome and Done have nothing to skip, and a station needs a modem. */
const REQUIRED: readonly WizardStep[] = ["welcome", "modem", "done"];

/** The steps on a path: My own Mercury's sound card and rig are set where it runs. */
export function stepsFor(path: WizardPath): WizardStep[] {
  return path === "existing" ? STEPS.filter((s) => s !== "audio" && s !== "rig") : [...STEPS];
}

/** The step after `step` on `path` (Done stays Done). */
export function next(step: WizardStep, path: WizardPath): WizardStep {
  const on = stepsFor(path);
  return on.find((s) => STEPS.indexOf(s) > STEPS.indexOf(step)) ?? "done";
}

/** The step before `step` on `path` (Welcome stays Welcome). */
export function back(step: WizardStep, path: WizardPath): WizardStep {
  const on = stepsFor(path);
  return [...on].reverse().find((s) => STEPS.indexOf(s) < STEPS.indexOf(step)) ?? "welcome";
}

export const canSkip = (step: WizardStep): boolean => !REQUIRED.includes(step);

/** Move past `state.step`, listing it as skipped (once); a required step stays. */
export function skip(state: WizardState, path: WizardPath): WizardState {
  if (!canSkip(state.step)) return state;
  const skipped = state.skipped.includes(state.step) ? state.skipped : [...state.skipped, state.step];
  return { ...state, step: next(state.step, path), skipped };
}
