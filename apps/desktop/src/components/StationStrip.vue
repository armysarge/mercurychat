<script setup lang="ts">
// The station strip across the top of every page (owner direction, 16 Nov):
//  1. Station: the radio's frequency in seven-segment digits, read-only from
//     telemetry with its age (dashes when there is none, never 0), band, mode,
//     bandwidth and the station switcher. M3b makes it the rig's tuning
//     control: tunable digits, band and mode menus and the frequency list,
//     with a "Rig" panel for the rest (RigPopover.vue).
//  2. Actions: Call CQ, Beacon now, Tune (refusals inline), and the beacon
//     countdown or why it is held; the PTT chips and the next scheduled QSY. Call / Disconnect / Abort stay in the
//     conversation header, which is always on screen (docs/04 §7), so a
//     collapsed strip never hides them.
//  3. Spectrum and waterfall (Waterfall.vue, Task 12): collapsed by default;
//     "telemetry unavailable" whenever its telemetry is not Up.
// Collapsible as one unit; the choice is remembered. Shown on every page, as
// one row no taller than 80 px (owner, 20 Nov); at 1100 px or less only the
// station and the actions show.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { BroadcastOutcome } from "../bindings/BroadcastOutcome";
import {
  activeProfile, app, discovery, discoveryError, inSession, modemState, selectProfile, sendBeaconNow, sendCq, snapshot,
} from "../store";
import type { FrequencyEntry } from "../bindings/FrequencyEntry";
import { stripOpen } from "../stripState";
import {
  currentBand, refusalText, rigRefusal, rigSet, rigUi, rigUp, rigView, setTune, shownBandwidth, shownFreq, shownMode, tuneToEntry, tuneView,
} from "../rig";
import { countdown } from "./countdown";
import FreqDigits from "./FreqDigits.vue";
import Popover from "./Popover.vue";
import RigPopover from "./RigPopover.vue";
import Waterfall from "./Waterfall.vue";
import { settingsTabRequest } from "../builtin";

const { t } = useI18n();
const props = defineProps<{ now: number }>();

const open = stripOpen;

// The built-in modem's own state (M5): a sound card to choose, starting, stopped.
const builtinDetail = computed(() => snapshot.value?.builtinDetail ?? null);
const lastLine = (s: string) => {
  const lines = s.split("\n").map((l) => l.trim()).filter(Boolean);
  return lines.length ? lines[lines.length - 1] : s;
};
function openAudio() {
  settingsTabRequest.value = "audio";
  app.page = "settings";
}
const beacon = computed(() => snapshot.value?.beacon ?? null);
const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
/** "7.105.000": the rig's frequency while its link is up, else Mercury's; dashes for none. */
const freqAge = computed(() => {
  const a = shownFreq.value?.ageMs;
  return a === undefined || a === null ? null : (a / 1000).toFixed(1);
});
const bandwidth = shownBandwidth;
const mode = shownMode;

// ---- the rig (M3b)
const rigConfigured = computed(() => !!rigView.value && rigView.value.link.type !== "unconfigured");
const freqRefusal = computed(() => refusalText(rigRefusal("frequency")));
const modeRefusal = computed(() => refusalText(rigRefusal("mode")));
const bandName = computed(() => currentBand.value ?? t("strip.noBand"));
const bandEntries = computed(() => rigUi.entries.filter((e) => e.band === currentBand.value));
const entryLabel = (e: FrequencyEntry) => t("strip.entry", { name: e.name, mhz: (e.freqHz / 1e6).toFixed(3) });
const bandBtn = ref<HTMLElement | null>(null);
const bandOpen = ref(false);
const modeBtn = ref<HTMLElement | null>(null);
const modeOpen = ref(false);
const moreBtn = ref<HTMLElement | null>(null);
const moreOpen = ref(false);
function pickEntry(id: string) {
  bandOpen.value = false;
  void tuneToEntry(id);
}
function pickMode(m: string) {
  modeOpen.value = false;
  void rigSet({ type: "mode", mode: m, passbandHz: 0 });
}
function onListPick(e: Event) {
  const el = e.target as HTMLSelectElement;
  const id = el.value;
  el.value = "";
  if (id) void tuneToEntry(id);
}
const tuneFreq = (hz: number) => void rigSet({ type: "frequency", hz });

// Tune: a confirmation the first time in a run (kept in memory only).
const tuneBtn = ref<HTMLElement | null>(null);
const confirmOpen = ref(false);
const tuneRefusal = computed(() => {
  const tv = tuneView.value;
  if (tv?.active) return "";
  if (tv?.supported === false) return t("strip.tuneNoSupport");
  if (!tv || tv.supported === null) return t("strip.tuneUnknown");
  return refusalText(rigRefusal("tune"));
});
const tuneLeft = computed(() => {
  const tv = tuneView.value;
  return tv?.active && tv.endsUtc ? countdown(tv.endsUtc, props.now).slice(3) : "";
});
function onTune() {
  if (tuneView.value?.active) return void setTune(false);
  if (!rigUi.tuneConfirmed) {
    confirmOpen.value = true;
    return;
  }
  void setTune(true);
}
function confirmTune() {
  rigUi.tuneConfirmed = true;
  confirmOpen.value = false;
  void setTune(true);
}
const nextQsy = computed(() => snapshot.value?.scheduleNext ?? null);
const fault = computed(() => rigView.value?.fault ?? null);
/** Something the operator must see even with the panel closed. */
const alerting = computed(
  () => builtinDetail.value?.kind === "chooseSoundCard" || builtinDetail.value?.kind === "mercuryStopped" || !!fault.value || !!rigUi.error,
);
const checkOff = computed(() => !!rigView.value?.pttCheckOff);
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
  <div class="strip-host">
  <!-- The station panel (owner, 23 Nov: no permanent bar): it drops from the header's station pill, which opens and closes it. -->
  <section v-show="open" id="station-panel" class="strip" :aria-label="t('strip.aria')">
    <div class="groups">
      <div class="group station">
        <div class="freq">
          <FreqDigits :hz="shownFreq?.hz ?? null" :age-ms="shownFreq?.ageMs ?? null" :editable="rigUp" :refusal="freqRefusal" @tune="tuneFreq" />
          <span v-if="freqAge !== null" class="muted small mono">{{ t("telemetry.ageS", { s: freqAge }) }}</span>
        </div>
        <div class="tags small">
          <button v-if="rigUp" ref="bandBtn" class="tag chip-btn band-chip mono" :aria-expanded="bandOpen" :title="t('strip.bandMenu', { band: bandName })" @click="bandOpen = !bandOpen">{{ bandName }}</button>
          <span v-else class="tag mono">{{ bandName }}</span>
          <button v-if="rigUp && rigView?.caps?.modes.length" ref="modeBtn" class="tag chip-btn mode-chip mono" :aria-expanded="modeOpen" :title="modeRefusal || t('strip.modeMenu')" :disabled="!!modeRefusal" @click="modeOpen = !modeOpen">{{ mode ?? t("strip.noMode") }}</button>
          <span v-else class="tag mono">{{ mode ?? t("strip.noMode") }}</span>
          <span class="tag mono">{{ bandwidth }}</span>
          <select v-if="rigConfigured" class="freq-list small" value="" :aria-label="t('strip.freqList')" :title="freqRefusal || t('strip.freqList')" :disabled="!!freqRefusal" @change="onListPick">
            <option value="">{{ t("strip.freqListPick") }}</option>
            <option v-for="e in rigUi.entries" :key="e.id" :value="e.id">{{ entryLabel(e) }}</option>
          </select>
          <select class="profile-switch small" :value="activeProfile?.id ?? ''" :aria-label="t('strip.profile')" :disabled="inSession" @change="selectProfile(($event.target as HTMLSelectElement).value)">
            <option v-for="p in snapshot?.profiles ?? []" :key="p.id" :value="p.id">{{ p.callsign }}</option>
          </select>
        </div>
      </div>

      <div class="group actions">
        <div class="buttons">
          <button class="cq small" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendCq">{{ t("strip.cq") }}</button>
          <button class="beacon-now small" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendBeaconNow">{{ t("strip.beaconNow") }}</button>
          <button v-if="activeProfile" ref="tuneBtn" :class="['tune', 'small', { danger: tuneView?.active }]" :disabled="!!tuneRefusal" :title="tuneRefusal || t('strip.tuneTitle')" @click="onTune">{{ tuneView?.active ? t("strip.tuneStop", { time: tuneLeft }) : t("strip.tune") }}</button>
          <button v-if="rigConfigured" ref="moreBtn" class="rig-more small" :aria-expanded="moreOpen" :title="t('strip.rigMore')" @click="moreOpen = !moreOpen">{{ t("strip.rig") }}</button>
          <span v-if="next" class="mono small next-beacon">{{ t("strip.nextBeacon", { time: next }) }}</span>
          <span v-else-if="beacon?.active && beacon.suppressed" class="warn small next-beacon line" :title="t('beacon.suppressed', { reason: beacon.suppressed })">{{ t("beacon.suppressed", { reason: beacon.suppressed }) }}</span>
          <span v-else class="muted small next-beacon">{{ t("beacon.off") }}</span>
        </div>
        <div class="status small">
          <button v-if="builtinDetail?.kind === 'chooseSoundCard'" type="button" class="builtin-note link warn line" @click="openAudio">{{ t("strip.chooseSoundCard") }}</button>
          <span v-else-if="builtinDetail?.kind === 'startingMercury'" class="builtin-note muted line" role="status">{{ t("strip.startingMercury") }}</span>
          <span v-else-if="builtinDetail?.kind === 'mercuryStopped'" class="builtin-note err line" role="alert" :title="builtinDetail.reason">{{ t("strip.mercuryStopped", { reason: lastLine(builtinDetail.reason) }) }}</span>
          <span v-if="fault" class="chip ptt-fault" role="alert" :title="t('strip.pttFaultTitle')">{{ t("strip.pttFault") }}</span>
          <span v-if="checkOff" class="chip ptt-check-off" :title="t('strip.pttCheckOffTitle')">{{ t("strip.pttCheckOff") }}</span>
          <span v-if="activeProfile && !idle" class="muted idle-only line" :title="t('strip.idleOnly')">{{ t("strip.idleOnly") }}</span>
          <span v-if="rigUi.error" class="warn rig-error line" role="alert" :title="rigUi.error">{{ rigUi.error }}</span>
          <span v-if="nextQsy" class="mono muted next-qsy line" :title="t('strip.nextQsy', { time: nextQsy.atUtc, name: nextQsy.entryName })">{{ t("strip.nextQsy", { time: nextQsy.atUtc, name: nextQsy.entryName }) }}</span>
          <span v-if="cqLine" :class="['cq-outcome', 'line', discoveryError.cq ? 'warn' : tone(discovery.outcome.cq)]" role="status" :title="t('strip.cqOutcome', { what: cqLine })">{{ t("strip.cqOutcome", { what: cqLine }) }}</span>
          <span v-if="beaconLine" :class="['beacon-outcome', 'line', tone(discovery.outcome.beacon)]" role="status" :title="t('strip.beaconOutcome', { what: beaconLine })">{{ t("strip.beaconOutcome", { what: beaconLine }) }}</span>
          <span v-if="discoveryError.beacon" class="warn beacon-error line" role="alert" :title="discoveryError.beacon">{{ discoveryError.beacon }}</span>
        </div>
      </div>

      <div v-if="!narrow" class="group spectrum">
        <Waterfall :shown="open && !narrow" />
      </div>
    </div>
    <Popover :open="bandOpen" :anchor="bandBtn" :label="t('strip.bandMenu', { band: bandName })" @close="bandOpen = false">
      <button v-for="e in bandEntries" :key="e.id" class="quiet small menu-item" :disabled="!!freqRefusal" :title="freqRefusal" @click="pickEntry(e.id)">{{ entryLabel(e) }}</button>
      <p v-if="!bandEntries.length" class="muted menu-empty">{{ t("strip.bandEmpty") }}</p>
    </Popover>
    <Popover :open="modeOpen" :anchor="modeBtn" :label="t('strip.modeMenu')" @close="modeOpen = false">
      <button v-for="m in rigView?.caps?.modes ?? []" :key="m" :class="['quiet', 'small', 'menu-item', 'mono', { current: m === mode }]" @click="pickMode(m)">{{ m }}</button>
    </Popover>
    <Popover :open="confirmOpen" :anchor="tuneBtn" :label="t('strip.tune')" @close="confirmOpen = false">
      <p class="menu-empty">{{ t("strip.tuneConfirm") }}</p>
      <div class="confirm-buttons">
        <button class="danger small tune-confirm" @click="confirmTune">{{ t("strip.tuneConfirmYes") }}</button>
        <button class="quiet small" @click="confirmOpen = false">{{ t("strip.cancel") }}</button>
      </div>
    </Popover>
    <RigPopover :open="moreOpen" :anchor="moreBtn" @close="moreOpen = false" />
    <button class="collapse quiet small" :title="t('strip.collapse')" @click="open = false">
      <span aria-hidden="true">▴</span><span class="sr-only">{{ t("strip.collapse") }}</span>
    </button>
  </section>
  <!-- With the panel closed, what needs the operator's eye still shows: a sound card to choose, Mercury stopped, a PTT fault, a refused rig command. -->
  <p v-if="!open && alerting" class="strip-alerts small">
    <button v-if="builtinDetail?.kind === 'chooseSoundCard'" type="button" class="link warn alert-sound-card" @click="openAudio">{{ t("strip.chooseSoundCard") }}</button>
    <span v-else-if="builtinDetail?.kind === 'mercuryStopped'" class="err line alert-mercury-stopped" role="alert" :title="builtinDetail.reason">{{ t("strip.mercuryStopped", { reason: lastLine(builtinDetail.reason) }) }}</span>
    <span v-if="fault" class="chip ptt-fault alert-ptt-fault" role="alert" :title="t('strip.pttFaultTitle')">{{ t("strip.pttFault") }}</span>
    <span v-if="rigUi.error" class="warn line alert-rig-error" role="alert" :title="rigUi.error">{{ rigUi.error }}</span>
  </p>
  </div>
</template>

<style scoped>
/* A panel over the workspace, never a bar in the layout (owner, 23 Nov): one row,
   never taller than 80 px: station, actions, then the spectrum at the right
   edge. Long status text is cut with an ellipsis and shown in full on hover. */
.strip-host { position: relative; flex: none; z-index: 25; }
.strip {
  position: absolute; top: 0; left: calc(var(--space) * 2); right: calc(var(--space) * 2);
  display: flex; align-items: center; gap: var(--space);
  box-sizing: border-box; height: 80px; max-height: 80px; overflow: hidden;
  padding: 6px var(--space) 6px calc(var(--space) * 2);
  background: var(--surface); border: 1px solid var(--border); border-top: 0;
  border-radius: 0 0 12px 12px; box-shadow: 0 10px 24px var(--shadow);
}
.strip-alerts {
  display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; margin: 0;
  padding: 4px calc(var(--space) * 2); background: var(--raised); border-bottom: 1px solid var(--border);
}
.link { min-height: 0; padding: 0; border: 0; background: transparent; font: inherit; color: var(--warning); }
.groups { flex: 1; min-width: 0; height: 100%; display: flex; align-items: center; gap: calc(var(--space) * 2); }
.group { display: flex; flex-direction: column; justify-content: center; gap: 6px; min-width: 0; height: 100%; }
.group + .group { border-left: 1px solid var(--border); padding-left: calc(var(--space) * 2); }
.station { flex: none; }
.actions { flex: 1 1 auto; }
.spectrum { flex: none; margin-left: auto; }
.freq { display: flex; align-items: baseline; gap: var(--space); line-height: 1; }
.big { font-size: calc(30px * var(--font-scale)); line-height: 1; }
.tags { display: flex; gap: 4px; align-items: center; flex-wrap: nowrap; }
.tag { padding: 1px 8px; border-radius: 999px; border: 1px solid var(--border-strong); color: var(--text-2); line-height: 1.5; white-space: nowrap; }
.profile-switch { max-width: 140px; padding: 0 4px; height: 24px; }
.buttons { display: flex; align-items: center; gap: 6px; min-width: 0; }
.next-beacon { margin-left: 6px; white-space: nowrap; }
.status { display: flex; gap: 12px; min-width: 0; height: 1.3em; }
.line { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.collapse { flex: none; border-color: transparent; padding: 0 8px; }
button.tag { background: transparent; font: inherit; height: auto; cursor: pointer; }
button.tag:disabled { cursor: not-allowed; }
.freq-list { max-width: 170px; padding: 0 4px; height: 24px; }
.chip { flex: none; padding: 0 6px; border-radius: 999px; border: 1px solid; white-space: nowrap; font-weight: 600; }
.ptt-fault { color: var(--danger); border-color: var(--danger); }
.ptt-check-off { color: var(--warning); border-color: var(--warning); }
.next-qsy { flex: none; }
.menu-item { text-align: left; justify-content: flex-start; }
.menu-item.current { color: var(--accent); }
.menu-empty { margin: 0; }
.confirm-buttons { display: flex; gap: 8px; justify-content: flex-end; }
</style>
