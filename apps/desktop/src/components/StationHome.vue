<script setup lang="ts">
// The idle station home (the design canvas, M5 Task 11b), shown in Operate
// while no conversation is open: the dial (the rig's frequency while its link
// is up, else Mercury's), the bands of the saved frequency list, the
// spectrum, the latest broadcasts, and the CQ, Broadcast and Beacon cards. Every control goes through the same
// commands as the station strip; nothing here transmits by itself.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { settingsTabRequest } from "../builtin";
import { currentBand, refusalText, rigRefusal, rigSet, rigUi, rigUp, rigView, shownBandwidth, shownFreq, shownMode, tuneToEntry } from "../rig";
import { activeProfile, app, discoveryError, modemState, sendCq } from "../store";
import BeaconControl from "./BeaconControl.vue";
import BroadcastMessage from "./BroadcastMessage.vue";
import FreqDigits from "./FreqDigits.vue";
import LatestBroadcasts from "./LatestBroadcasts.vue";
import Waterfall from "./Waterfall.vue";

const { t } = useI18n();
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | undefined;
onMounted(() => (clock = setInterval(() => (now.value = Date.now()), 1000)));
onUnmounted(() => clearInterval(clock));

const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
const freqRefusal = computed(() => refusalText(rigRefusal("frequency")));
const source = computed(() => {
  if (!shownFreq.value) return t("home.sourceNone");
  return rigUp.value && rigView.value?.frequency ? t("home.sourceRig") : t("home.sourceMercury");
});

/** The bands of the saved frequency list, in the list's order; a band tunes to its first entry. */
const bands = computed(() => {
  const seen = new Map<string, string>();
  for (const e of rigUi.entries) if (e.band && !seen.has(e.band)) seen.set(e.band, e.id);
  return [...seen].map(([band, id]) => ({ band, id }));
});
const tuneFreq = (hz: number) => void rigSet({ type: "frequency", hz });
/** The saved frequency list as a dropdown (owner, 24 Nov): picking one tunes to it. */
const picked = ref("");
function pick() {
  if (picked.value) void tuneToEntry(picked.value);
  picked.value = "";
}
const entryText = (e: { name: string; freqHz: number }) => `${e.name} · ${(e.freqHz / 1e6).toFixed(3)} MHz`;
function savedFrequencies() {
  settingsTabRequest.value = "rig";
  app.page = "settings";
}
</script>

<template>
  <section class="station-home" :aria-label="t('home.aria')">
    <div class="card dial">
      <span class="label">{{ t("home.dial") }}</span>
      <FreqDigits class="big" :hz="shownFreq?.hz ?? null" :age-ms="shownFreq?.ageMs ?? null" :editable="rigUp" :refusal="freqRefusal" @tune="tuneFreq" />
      <div class="chips small">
        <span class="tag mono">{{ currentBand ?? t("strip.noBand") }}</span>
        <span class="tag mono">{{ shownMode ?? t("strip.noMode") }}</span>
        <span class="tag mono">{{ shownBandwidth }}</span>
      </div>
      <p class="muted small source">{{ source }}</p>
      <p class="small tune-hint muted">{{ rigUp && !freqRefusal ? t("home.tuneHint") : t("home.noRigHint") }}</p>
      <label v-if="rigUi.entries.length" class="freq-pick small">
        {{ t("home.pickFreq") }}
        <select v-model="picked" class="saved-frequency" :disabled="!!freqRefusal" :title="freqRefusal || undefined" @change="pick">
          <option value="" disabled>{{ t("home.pickPlaceholder") }}</option>
          <option v-for="e in rigUi.entries" :key="e.id" :value="e.id">{{ entryText(e) }}</option>
        </select>
      </label>
    </div>

    <div class="card bands">
      <span class="label">{{ t("home.bands") }}</span>
      <div v-if="bands.length" class="band-buttons">
        <button
          v-for="b in bands"
          :key="b.band"
          :class="['band', 'mono', 'small', { current: b.band === currentBand }]"
          :aria-pressed="b.band === currentBand"
          :disabled="!!freqRefusal"
          :title="freqRefusal || t('home.tuneBand', { band: b.band })"
          @click="tuneToEntry(b.id)"
        >{{ b.band }}</button>
      </div>
      <p v-else class="muted small">{{ t("home.noBands") }}</p>
      <button class="link small saved-frequencies" @click="savedFrequencies">{{ t("home.savedFrequencies") }}</button>
      <p v-if="rigUi.error" class="warn small" role="alert">{{ rigUi.error }}</p>
    </div>

    <div class="card spectrum">
      <Waterfall :shown="true" large />
    </div>

    <LatestBroadcasts class="card latest" :title="t('latest.title')" />

    <div class="card cq-card">
      <span class="label">{{ t("home.cqTitle") }}</span>
      <p class="muted small">{{ t("home.cqBody") }}</p>
      <button class="primary send-cq" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendCq">{{ t("home.cqNow") }}</button>
      <p v-if="discoveryError.cq" class="warn small" role="alert">{{ discoveryError.cq }}</p>
    </div>

    <div class="card broadcast-card">
      <span class="label">{{ t("broadcast.title") }}</span>
      <BroadcastMessage />
      <p class="muted small">{{ t("broadcast.hint") }}</p>
    </div>

    <div class="beacon-card">
      <BeaconControl :now="now" />
    </div>
  </section>
</template>

<style scoped>
.station-home {
  flex: 1; min-height: 0; overflow-y: auto; padding: calc(var(--space) * 2);
  display: grid; gap: calc(var(--space) * 1.5); align-content: start;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  grid-template-areas: "dial bands" "spectrum spectrum" "latest latest" "cq beacon" "broadcast beacon";
}
.card { border-radius: 12px; min-width: 0; }
.dial { grid-area: dial; display: grid; gap: 6px; align-content: start; }
.dial .big { font-size: calc(48px * var(--font-scale)); line-height: 1.1; }
.tune-hint { margin: 0; }
.freq-pick { display: grid; gap: 4px; color: var(--text-2); max-width: 360px; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.source { margin: 0; }
.tag { padding: 1px 8px; border-radius: 999px; border: 1px solid var(--border-strong); color: var(--text-2); line-height: 1.5; white-space: nowrap; }
.link { min-height: 0; padding: 0; border: 0; background: transparent; font: inherit; color: var(--accent); }
.bands { grid-area: bands; display: grid; gap: var(--space); align-content: start; justify-items: start; }
.band-buttons { display: flex; flex-wrap: wrap; gap: 6px; }
.band { min-width: 56px; border-radius: 999px; }
.band.current { border-color: var(--accent); color: var(--accent); background: var(--raised); }
.spectrum { grid-area: spectrum; }
.latest { grid-area: latest; }
.cq-card { grid-area: cq; display: grid; gap: var(--space); align-content: start; justify-items: start; }
.cq-card p { margin: 0; }
.broadcast-card { grid-area: broadcast; display: grid; gap: var(--space); align-content: start; }
.broadcast-card p { margin: 0; }
.beacon-card { grid-area: beacon; min-width: 0; }
.beacon-card :deep(.card) { border-radius: 12px; height: 100%; }
@media (max-width: 900px) {
  .station-home { grid-template-columns: minmax(0, 1fr); grid-template-areas: "dial" "bands" "spectrum" "latest" "cq" "broadcast" "beacon"; padding: calc(var(--space) * 1.5); }
}
</style>
