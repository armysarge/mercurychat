// M3c's operator extras in the window: QSY during a session and before one,
// information requests, the extended beacon and what the station shares.
// Every change goes through a typed command; the core decides (B1–B8).
import { computed, reactive, watch } from "vue";
import { call, describeError } from "./api";
import type { BeaconContentView } from "./bindings/BeaconContentView";
import type { PeerSnr } from "./bindings/PeerSnr";
import type { PeerStation } from "./bindings/PeerStation";
import type { ShareField } from "./bindings/ShareField";
import type { Tuning } from "./bindings/Tuning";
import { activeProfile, onAppEvent, session, snapshot } from "./store";

/** The MCH/1 capabilities M3c's session controls need (B1). */
export const CAP_QSY = "qsy/1";
export const CAP_INFO = "info/1";
/** The session's peer listed `cap` too. */
export const peerHas = (cap: string) => !!session.value?.caps.includes(cap);

export const qsy = computed(() => snapshot.value?.qsy ?? null);

export const extras = reactive({
  /** When the QSY card's view arrived: its countdown runs from here. */
  qsyAt: Date.now(),
  /** The latest QSY notice (declined, no answer, fell back). */
  notice: null as null | { key: string; peer: string; original: string | null },
  /** The last suggestion addressed to us (MCB/1 kind 3), until dismissed. */
  suggestion: null as null | { from: string; candidates: Tuning[] },
  /** The peer's last answer to an information request, or its shared email. */
  peerInfo: null as null | { peer: string; snr: PeerSnr | null; station: PeerStation | null; reason: string | null; at: number },
  /** The last refusal of one of these commands, shown beside the controls. */
  error: "",
});

watch(
  () => snapshot.value?.qsy,
  (v) => {
    extras.qsyAt = Date.now();
    if (v) extras.notice = null;
  },
);
// A new session starts with nothing reported yet.
watch(
  () => session.value?.sessionId,
  () => (extras.peerInfo = null),
);

onAppEvent((e) => {
  if (e.type === "qsyNotice") extras.notice = { key: e.key, peer: e.peer, original: e.original };
  else if (e.type === "qsySuggested") extras.suggestion = { from: e.from, candidates: e.candidates };
  else if (e.type === "peerInfo") extras.peerInfo = { peer: e.peer, snr: e.snr, station: e.station, reason: e.reason, at: Date.now() };
});

async function extra(step: () => Promise<unknown>): Promise<boolean> {
  extras.error = "";
  try {
    await step();
    return true;
  } catch (e) {
    extras.error = describeError(e);
    return false;
  }
}

export const proposeQsy = (entryIds: string[], note: string | null) => extra(() => call({ type: "proposeQsy", entryIds, note }));
export const answerQsy = (accept: number | null) => extra(() => call({ type: "answerQsy", accept }));
export const qsyTuned = () => extra(() => call({ type: "qsyTuned" }));
export const requestInfo = (what: "snr" | "station") => extra(() => call({ type: "requestInfo", what }));
export const sendEmail = () => extra(() => call({ type: "sendEmail" }));
export const suggestQsy = (to: string, entryIds: string[]) => extra(() => call({ type: "suggestQsy", to, entryIds }));
export const tuneSuggested = (index: number) => extra(() => call({ type: "tuneSuggested", index }));

/** Seconds left on the QSY card's window at `now` (null when none runs). */
export function qsyLeft(now: number): number | null {
  const left = qsy.value?.secondsLeft;
  if (left === null || left === undefined) return null;
  return Math.max(0, left - Math.floor((now - extras.qsyAt) / 1000));
}

/** "7.105.000 USB", the way the rig strip shows a frequency. */
export function describeTuning(t: { freqHz: number; mode: string }): string {
  const hz = t.freqHz;
  const f = `${Math.floor(hz / 1e6)}.${String(Math.floor(hz / 1e3) % 1000).padStart(3, "0")}.${String(hz % 1000).padStart(3, "0")}`;
  return t.mode ? `${f} ${t.mode}` : f;
}

// ---- Settings → Beacon (B7) and Settings → Station → Share (B6)

export async function beaconContent(): Promise<BeaconContentView | null> {
  if (!activeProfile.value) return null;
  const r = await call({ type: "beaconContent" });
  return r.type === "beaconContent" ? r : null;
}

export async function previewBeacon(name: boolean, qth: boolean, status: string | null): Promise<number | null> {
  const r = await call({ type: "previewBeaconFields", name, qth, status });
  return r.type === "beaconContent" ? r.bytesLeft : null;
}

export async function saveBeaconFields(name: boolean, qth: boolean, status: string | null): Promise<BeaconContentView | null> {
  const r = await call({ type: "setBeaconFields", name, qth, status });
  return r.type === "beaconContent" ? r : null;
}

export const SHARE_FIELDS: readonly ShareField[] = ["name", "qth", "locator", "rig", "power", "antenna", "about"];

export async function shareSettings(): Promise<ShareField[] | null> {
  if (!activeProfile.value) return null;
  const r = await call({ type: "shareSettings" });
  return r.type === "share" ? r.fields : null;
}

export async function setShareSettings(fields: ShareField[]): Promise<ShareField[] | null> {
  const r = await call({ type: "setShareSettings", config: { fields } });
  return r.type === "share" ? r.fields : null;
}
