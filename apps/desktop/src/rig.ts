// The rig as the station strip and Settings use it (M3b): the core's rig and
// Tune state from the snapshot, the frequency list, the core's gate as far as
// the window can see it (so a control is disabled with its reason before it
// is pressed), and the commands. Every change still goes through the core,
// which decides at the moment it forwards it; a refusal is shown inline.
import { computed, reactive, watch } from "vue";
import { call, describeError } from "./api";
import type { BandLevels } from "./bindings/BandLevels";
import type { FrequencyEntry } from "./bindings/FrequencyEntry";
import type { RigBanner } from "./bindings/RigBanner";
import type { RigChange } from "./bindings/RigChange";
import { bandFor } from "./components/freqStep";
import { t } from "./i18n";
import { activeProfile, modemState, onAppEvent, snapshot } from "./store";

export const rigView = computed(() => snapshot.value?.rig ?? null);
export const tuneView = computed(() => snapshot.value?.tune ?? null);
export const rigUp = computed(() => rigView.value?.link.type === "up");
/** The rig's dial frequency in Hz, while the link is up. */
export const rigFreq = computed(() => {
  const f = rigView.value?.frequency;
  return rigUp.value && f ? Number(f.value) : null;
});

/** The core's gate reasons (RigRefusal::key, `rig.refused.*`). */
export const REFUSALS = [
  "calling", "connected", "draining", "tuning", "broadcasting", "pttOn", "pttUnknown", "linkDown", "unsupported", "notAttached",
] as const;
export type Refusal = (typeof REFUSALS)[number];

export type RigControl = "frequency" | "mode" | "vfo" | "split" | "rfPower" | "tune" | "txLevel";

/** The `rig.refused.*` reason the core's gate would give for `control` now, or null. */
export function rigRefusal(control: RigControl): Refusal | null {
  const r = rigView.value;
  switch (modemState.value) {
    case "calling":
    case "negotiating":
      return "calling";
    case "connected":
      return "connected";
    case "draining":
    case "recovering":
      return "draining";
  }
  if (tuneView.value?.active) return "tuning";
  if (r?.ptt === "on") return "pttOn";
  const viaModem = control === "tune" || control === "txLevel";
  if (!viaModem && r?.link.type !== "up") return "linkDown";
  if (control === "tune" && r && r.link.type !== "unconfigured" && r.ptt === "unknown") return "pttUnknown";
  if (viaModem && !["ready", "listening"].includes(modemState.value)) return "notAttached";
  const c = r?.caps;
  const supported: Record<RigControl, boolean> = {
    frequency: !!c?.canSetFreq,
    mode: !!c?.canSetMode,
    vfo: !!c?.canSetVfo,
    split: !!c?.canSplit,
    rfPower: !!c?.canSetPower,
    tune: true,
    txLevel: true,
  };
  return supported[control] ? null : "unsupported";
}

/** The refusal in words, for a disabled control's title. */
export const refusalText = (reason: string | null) => (reason ? t(`rig.refused.${reason}`) : "");

export const rigUi = reactive({
  /** The last rig command's refusal, shown inline in the strip. */
  error: "",
  entries: [] as FrequencyEntry[],
  levels: {} as Record<string, BandLevels>,
  /** Tune's transmit warning was accepted this run (never stored). */
  tuneConfirmed: false,
});

async function rigAct(step: () => Promise<unknown>): Promise<boolean> {
  rigUi.error = "";
  try {
    await step();
    return true;
  } catch (e) {
    rigUi.error = describeError(e);
    return false;
  }
}

export const rigSet = (change: RigChange) => rigAct(() => call({ type: "rigSet", change }));
export const setTune = (on: boolean) => rigAct(() => call({ type: "tune", on }));
export const setTuneLevel = (dbfs: number) => rigAct(() => call({ type: "setTuneLevel", dbfs }));
export const tuneToEntry = (id: string) => rigAct(() => call({ type: "tuneToEntry", id }));
export const dismissRigBanner = (banner: RigBanner) => rigAct(() => call({ type: "dismissRigBanner", banner }));
export async function setBandLevels(band: string, levels: BandLevels) {
  const ok = await rigAct(() => call({ type: "setBandLevels", band, levels }));
  if (ok) rigUi.levels = { ...rigUi.levels, [band]: levels };
  return ok;
}

export async function loadFrequencies() {
  if (!activeProfile.value) {
    rigUi.entries = [];
    return;
  }
  try {
    const r = await call({ type: "frequencies" });
    if (r.type === "frequencies") rigUi.entries = r.entries;
  } catch {
    // Shown as an empty list; the next change event reads it again.
  }
}

export async function loadBandLevels() {
  if (!activeProfile.value) {
    rigUi.levels = {};
    return;
  }
  try {
    const r = await call({ type: "bandLevels" });
    if (r.type === "bandLevels") rigUi.levels = r.levels as Record<string, BandLevels>;
  } catch {
    rigUi.levels = {};
  }
}

watch(
  () => activeProfile.value?.id,
  () => {
    void loadFrequencies();
    void loadBandLevels();
  },
  { immediate: true },
);
onAppEvent((e) => {
  if (e.type === "frequenciesChanged") void loadFrequencies();
});

/** The dial frequency shown: the rig's while its link is up, else Mercury's telemetry. */
export const shownFreq = computed(() => {
  if (rigFreq.value !== null) return { hz: rigFreq.value, ageMs: rigView.value?.frequency?.ageMs ?? null };
  const f = snapshot.value?.telemetry.frequency;
  return f ? { hz: f.value, ageMs: f.ageMs } : null;
});

/** The band the station is on, from the frequency shown. */
export const currentBand = computed(() => bandFor(shownFreq.value?.hz));
