<script setup lang="ts">
// The station strip across the top of Operate (owner direction, 16 Nov):
//  1. Station: the radio's frequency in seven-segment digits, read-only from
//     telemetry with its age (dashes when there is none, never 0), band, mode,
//     bandwidth and the station switcher. M3b makes it the rig's tuning control.
//  2. Actions: Call CQ, Beacon now (refusals inline), and the beacon
//     countdown or why it is held. Call / Disconnect / Abort stay in the
//     conversation header, which is always on screen (docs/04 §7), so a
//     collapsed strip never hides them.
//  3. Spectrum: Task 12; until then, and whenever its telemetry is not Up,
//     "telemetry unavailable".
// Collapsible as one unit; the choice is remembered. Narrow windows wrap it,
// and at 960 px or less only the station and the actions show.
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { BroadcastOutcome } from "../bindings/BroadcastOutcome";
import {
  activeProfile, discovery, discoveryError, inSession, modemState, selectProfile, sendBeaconNow, sendCq, session, snapshot,
} from "../store";
import { countdown } from "./countdown";

const { t } = useI18n();
const props = defineProps<{ now: number }>();

const KEY = "mc.stripOpen";
const open = ref((() => {
  try {
    return localStorage.getItem(KEY) !== "0";
  } catch {
    return true;
  }
})());
watch(open, (v) => {
  try {
    localStorage.setItem(KEY, v ? "1" : "0");
  } catch {
    // Storage may be unavailable: the choice then lasts for this run only.
  }
});

const tel = computed(() => snapshot.value?.telemetry ?? null);
const beacon = computed(() => snapshot.value?.beacon ?? null);
const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
/** "7.105.000" from Hz; dashes for none. */
const digits = computed(() => {
  const f = tel.value?.frequency?.value;
  if (f === undefined || f === null) return "--.---.---";
  const hz = Math.round(f);
  const khz = String(Math.floor((hz % 1_000_000) / 1000)).padStart(3, "0");
  return `${Math.floor(hz / 1_000_000)}.${khz}.${String(hz % 1000).padStart(3, "0")}`;
});
const freqAria = computed(() => {
  const f = tel.value?.frequency?.value;
  return f === undefined || f === null ? t("strip.freqNone") : t("strip.freqAria", { mhz: (f / 1e6).toFixed(3) });
});
const freqAge = computed(() => {
  const a = tel.value?.frequency?.ageMs;
  return a === undefined || a === null ? null : (a / 1000).toFixed(1);
});
const bandwidth = computed(() => (session.value?.bandwidth ? `BW${session.value.bandwidth}` : "BW2300"));
const mode = computed(() => tel.value?.txMode ?? tel.value?.rxMode ?? null);
const next = computed(() => (beacon.value?.active && beacon.value.nextDueUtc ? countdown(beacon.value.nextDueUtc, props.now) : null));

/** A CQ or beacon outcome in words: "transmitted", never "delivered" (DSC-02). */
function outcomeText(o: BroadcastOutcome | undefined): string {
  if (!o) return "";
  if (o.type === "refused") return t("strip.outcome.refused", { reason: o.reason });
  if (o.type === "notConfirmed") return t(o.reason ? `broadcast.notConfirmed.${o.reason}` : "strip.outcome.notConfirmed");
  return t(`strip.outcome.${o.type}`);
}
const cqLine = computed(() => discoveryError.cq || outcomeText(discovery.outcome.cq));
const beaconLine = computed(() => outcomeText(discovery.outcome.beacon));
const tone = (o: BroadcastOutcome | undefined) => (o?.type === "transmitted" ? "ok" : o?.type === "submitted" ? "muted" : "warn");
const spectrumUp = computed(() => tel.value?.link.type === "up");
</script>

<template>
  <section :class="['strip', { closed: !open }]" :aria-label="t('strip.aria')">
    <button class="collapse quiet" :aria-expanded="open" aria-controls="strip-body" :title="t(open ? 'strip.collapse' : 'strip.expand')" @click="open = !open">
      <span aria-hidden="true">{{ open ? "▴" : "▾" }}</span><span class="sr-only">{{ t(open ? "strip.collapse" : "strip.expand") }}</span>
    </button>
    <div v-show="open" id="strip-body" class="groups">
      <div class="group station">
        <div class="freq">
          <span class="freq-digits big" role="img" :aria-label="freqAria" :title="freqAge !== null ? t('telemetry.ageS', { s: freqAge }) : t('strip.freqNone')">{{ digits }}</span>
          <span v-if="freqAge !== null" class="muted small mono">{{ t("telemetry.ageS", { s: freqAge }) }}</span>
        </div>
        <div class="tags small">
          <span class="tag mono">{{ tel?.band ?? t("strip.noBand") }}</span>
          <span class="tag mono">{{ mode ?? t("strip.noMode") }}</span>
          <span class="tag mono">{{ bandwidth }}</span>
        </div>
        <select class="profile-switch small" :value="activeProfile?.id ?? ''" :aria-label="t('strip.profile')" :disabled="inSession" @change="selectProfile(($event.target as HTMLSelectElement).value)">
          <option v-for="p in snapshot?.profiles ?? []" :key="p.id" :value="p.id">{{ p.callsign }}</option>
        </select>
      </div>

      <div class="group actions">
        <div class="row buttons">
          <button class="cq" :disabled="!activeProfile || !idle" @click="sendCq">{{ t("strip.cq") }}</button>
          <button class="beacon-now" :disabled="!activeProfile || !idle" @click="sendBeaconNow">{{ t("strip.beaconNow") }}</button>
        </div>
        <div class="row status small">
          <span v-if="cqLine" :class="['cq-outcome', discoveryError.cq ? 'warn' : tone(discovery.outcome.cq)]" role="status">{{ t("strip.cqOutcome", { what: cqLine }) }}</span>
          <span v-if="beaconLine" :class="['beacon-outcome', tone(discovery.outcome.beacon)]" role="status">{{ t("strip.beaconOutcome", { what: beaconLine }) }}</span>
          <span v-if="discoveryError.beacon" class="warn beacon-error" role="alert">{{ discoveryError.beacon }}</span>
          <span class="grow"></span>
          <span v-if="next" class="mono next-beacon">{{ t("strip.nextBeacon", { time: next }) }}</span>
          <span v-else-if="beacon?.active && beacon.suppressed" class="warn next-beacon">{{ t("beacon.suppressed", { reason: beacon.suppressed }) }}</span>
          <span v-else class="muted next-beacon">{{ t("beacon.off") }}</span>
        </div>
      </div>

      <div class="group spectrum">
        <span class="label">{{ t("strip.spectrum") }}</span>
        <p class="muted small">{{ spectrumUp ? t("strip.spectrumSoon") : t("strip.telemetryUnavailable") }}</p>
      </div>
    </div>
    <p v-show="!open" class="closed-line small">
      <span class="freq-digits">{{ digits }}</span>
      <span v-if="next" class="mono muted">{{ t("strip.nextBeacon", { time: next }) }}</span>
    </p>
  </section>
</template>

<style scoped>
.strip {
  position: relative; display: block; flex: none;
  padding: var(--space) calc(var(--space) * 5) var(--space) calc(var(--space) * 2);
  background: var(--surface); border-bottom: 1px solid var(--border);
}
.collapse { position: absolute; top: 6px; right: 6px; padding: 2px 8px; border-color: transparent; }
.groups { display: grid; grid-template-columns: auto minmax(0, 1fr) minmax(180px, 260px); gap: calc(var(--space) * 2); align-items: center; }
.group { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.group + .group { border-left: 1px solid var(--border); padding-left: calc(var(--space) * 2); }
.freq { display: flex; align-items: baseline; gap: var(--space); }
.big { font-size: calc(28px * var(--font-scale)); line-height: 1.1; }
.tags { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
.tag { padding: 1px 8px; border-radius: 999px; border: 1px solid var(--border-strong); color: var(--text-2); }
.profile-switch { max-width: 220px; align-self: flex-start; }
.buttons { flex-wrap: wrap; }
.status { gap: 4px 12px; min-height: 1.4em; }
.closed-line { display: flex; align-items: center; gap: calc(var(--space) * 2); margin: 0; }
@media (max-width: 1279px) {
  .groups { grid-template-columns: auto minmax(0, 1fr); }
  .spectrum { grid-column: 1 / -1; border-left: 0; padding-left: 0; border-top: 1px solid var(--border); padding-top: 6px; }
}
@media (max-width: 960px) {
  .spectrum { display: none; }
  .groups { grid-template-columns: minmax(0, 1fr); }
  .group + .group { border-left: 0; padding-left: 0; }
}
</style>

