<script setup lang="ts">
// The station strip across the top of every page (owner direction, 16 Nov):
//  1. Station: the radio's frequency in seven-segment digits, read-only from
//     telemetry with its age (dashes when there is none, never 0), band, mode,
//     bandwidth and the station switcher. M3b makes it the rig's tuning control.
//  2. Actions: Call CQ, Beacon now (refusals inline), and the beacon
//     countdown or why it is held. Call / Disconnect / Abort stay in the
//     conversation header, which is always on screen (docs/04 §7), so a
//     collapsed strip never hides them.
//  3. Spectrum and waterfall (Waterfall.vue, Task 12): collapsed by default;
//     "telemetry unavailable" whenever its telemetry is not Up.
// Collapsible as one unit; the choice is remembered. Shown on every page, as
// one row no taller than 60 px (owner, 17 Nov); at 1100 px or less only the
// station and the actions show.
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { BroadcastOutcome } from "../bindings/BroadcastOutcome";
import {
  activeProfile, discovery, discoveryError, inSession, modemState, selectProfile, sendBeaconNow, sendCq, session, snapshot,
} from "../store";
import { countdown } from "./countdown";
import Waterfall from "./Waterfall.vue";

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
  // The core's reason already starts "not sent: "; the key adds those words in the UI's language.
  if (o.type === "refused") return t("strip.outcome.refused", { reason: o.reason.replace(/^not sent: /, "") });
  if (o.type === "notConfirmed") return t(o.reason ? `broadcast.notConfirmed.${o.reason}` : "strip.outcome.notConfirmed");
  return t(`strip.outcome.${o.type}`);
}
const cqLine = computed(() => discoveryError.cq || outcomeText(discovery.outcome.cq));
const beaconLine = computed(() => outcomeText(discovery.outcome.beacon));
const tone = (o: BroadcastOutcome | undefined) => (o?.type === "transmitted" ? "ok" : o?.type === "submitted" ? "muted" : "warn");
// At 1100 px or less the spectrum group is not shown: nothing is forwarded then.
const narrowQuery = window.matchMedia("(max-width: 1100px)");
const narrow = ref(narrowQuery.matches);
const onNarrow = (e: MediaQueryListEvent) => (narrow.value = e.matches);
onMounted(() => narrowQuery.addEventListener("change", onNarrow));
onUnmounted(() => narrowQuery.removeEventListener("change", onNarrow));
</script>

<template>
  <section :class="['strip', { closed: !open }]" :aria-label="t('strip.aria')">
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
          <select class="profile-switch small" :value="activeProfile?.id ?? ''" :aria-label="t('strip.profile')" :disabled="inSession" @change="selectProfile(($event.target as HTMLSelectElement).value)">
            <option v-for="p in snapshot?.profiles ?? []" :key="p.id" :value="p.id">{{ p.callsign }}</option>
          </select>
        </div>
      </div>

      <div class="group actions">
        <div class="buttons">
          <button class="cq small" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendCq">{{ t("strip.cq") }}</button>
          <button class="beacon-now small" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendBeaconNow">{{ t("strip.beaconNow") }}</button>
          <span v-if="next" class="mono small next-beacon">{{ t("strip.nextBeacon", { time: next }) }}</span>
          <span v-else-if="beacon?.active && beacon.suppressed" class="warn small next-beacon line" :title="t('beacon.suppressed', { reason: beacon.suppressed })">{{ t("beacon.suppressed", { reason: beacon.suppressed }) }}</span>
          <span v-else class="muted small next-beacon">{{ t("beacon.off") }}</span>
        </div>
        <div class="status small">
          <span v-if="activeProfile && !idle" class="muted idle-only line" :title="t('strip.idleOnly')">{{ t("strip.idleOnly") }}</span>
          <span v-if="cqLine" :class="['cq-outcome', 'line', discoveryError.cq ? 'warn' : tone(discovery.outcome.cq)]" role="status" :title="t('strip.cqOutcome', { what: cqLine })">{{ t("strip.cqOutcome", { what: cqLine }) }}</span>
          <span v-if="beaconLine" :class="['beacon-outcome', 'line', tone(discovery.outcome.beacon)]" role="status" :title="t('strip.beaconOutcome', { what: beaconLine })">{{ t("strip.beaconOutcome", { what: beaconLine }) }}</span>
          <span v-if="discoveryError.beacon" class="warn beacon-error line" role="alert" :title="discoveryError.beacon">{{ discoveryError.beacon }}</span>
        </div>
      </div>

      <div v-if="!narrow" class="group spectrum">
        <Waterfall :shown="open && !narrow" />
      </div>
    </div>
    <p v-show="!open" class="closed-line small">
      <span class="freq-digits">{{ digits }}</span>
      <span v-if="next" class="mono muted">{{ t("strip.nextBeacon", { time: next }) }}</span>
    </p>
    <button class="collapse quiet small" :aria-expanded="open" aria-controls="strip-body" :title="t(open ? 'strip.collapse' : 'strip.expand')" @click="open = !open">
      <span aria-hidden="true">{{ open ? "▴" : "▾" }}</span><span class="sr-only">{{ t(open ? "strip.collapse" : "strip.expand") }}</span>
    </button>
  </section>
</template>

<style scoped>
/* One row, never taller than 60 px (owner, 17 Nov): station, actions, then
   the spectrum at the right edge. Long status text is cut with an ellipsis
   and shown in full on hover. */
.strip {
  display: flex; align-items: center; gap: var(--space); flex: none;
  box-sizing: border-box; height: 60px; max-height: 60px; overflow: hidden;
  padding: 4px var(--space) 4px calc(var(--space) * 2);
  background: var(--surface); border-bottom: 1px solid var(--border);
}
.strip.closed { height: 32px; }
.groups { flex: 1; min-width: 0; height: 100%; display: flex; align-items: center; gap: calc(var(--space) * 2); }
.group { display: flex; flex-direction: column; justify-content: center; gap: 2px; min-width: 0; height: 100%; }
.group + .group { border-left: 1px solid var(--border); padding-left: calc(var(--space) * 2); }
.station { flex: none; }
.actions { flex: 1 1 auto; }
.spectrum { flex: none; margin-left: auto; }
.freq { display: flex; align-items: baseline; gap: var(--space); line-height: 1; }
.big { font-size: calc(22px * var(--font-scale)); line-height: 1; }
.tags { display: flex; gap: 4px; align-items: center; flex-wrap: nowrap; }
.tag { padding: 0 6px; border-radius: 999px; border: 1px solid var(--border-strong); color: var(--text-2); line-height: 1.4; white-space: nowrap; }
.profile-switch { max-width: 140px; padding: 0 4px; height: 20px; }
.buttons { display: flex; align-items: center; gap: 6px; min-width: 0; }
.next-beacon { margin-left: 6px; white-space: nowrap; }
.status { display: flex; gap: 12px; min-width: 0; height: 1.3em; }
.line { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.collapse { flex: none; border-color: transparent; padding: 0 8px; }
.closed-line { flex: 1; display: flex; align-items: center; gap: calc(var(--space) * 2); margin: 0; }
</style>
