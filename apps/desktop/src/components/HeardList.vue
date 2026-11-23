<script setup lang="ts">
// The sidebar's Heard tab (DSC-01, DSC-04; docs/04 §2, §8.1): stations heard
// by CQ, beacon or session, newest first. Each row says how it was heard in
// words, its age (UTC on hover), its locator and SNR, with the source of each
// reading on hover. A missing reading shows nothing, never "0". Selecting a
// row fills the peer (opens the conversation); calling stays a separate button.
// Heard is not the same as reachable.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { HeardSource } from "../bindings/HeardSource";
import type { HeardStation } from "../bindings/HeardStation";
import type { Reading } from "../bindings/Reading";
import Avatar from "./Avatar.vue";
import QsyPropose from "./QsyPropose.vue";
import { describeTuning, extras, tuneSuggested } from "../extras";
import { activeProfile, app, chat, discovery, discoveryError, inSession, linkPeer, openPeer, setFavourite } from "../store";

const { t } = useI18n();
const emit = defineEmits<{ opened: [peer: string] }>();
const query = ref("");
const filter = ref<"all" | HeardSource | "favourite">("all");
const FILTERS = ["all", "cq", "beacon", "session", "favourite"] as const;

// The ages tick once a second while the list is on screen.
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | undefined;
onMounted(() => (clock = setInterval(() => (now.value = Date.now()), 1000)));
onUnmounted(() => clearInterval(clock));

const shown = computed(() => {
  const q = query.value.trim().toUpperCase();
  return discovery.heard.filter((h) => {
    if (q && !h.callsign.includes(q) && !(h.grid ?? "").toUpperCase().includes(q)) return false;
    if (filter.value === "favourite") return h.favourite;
    return filter.value === "all" || h.source === filter.value;
  });
});

/** "12 s", "4 min", "3 h", "2 d" since `utc`. */
function age(utc: string): string {
  const s = Math.max(0, Math.floor((now.value - Date.parse(utc)) / 1000));
  if (s < 60) return t("heard.age.s", { n: s });
  if (s < 3600) return t("heard.age.min", { n: Math.floor(s / 60) });
  if (s < 86400) return t("heard.age.h", { n: Math.floor(s / 3600) });
  return t("heard.age.d", { n: Math.floor(s / 86400) });
}
const utc = (iso: string) => iso.slice(0, 10) + " " + iso.slice(11, 19) + " UTC";

/** Where a reading came from and how old it was, for its tooltip. */
function provenance(r: Reading<number>): string {
  const at = utc(r.atUtc);
  if (r.ageMs === null) return t("heard.from", { source: r.source, at });
  return t("heard.fromAged", { source: r.source, at, age: (r.ageMs / 1000).toFixed(1) });
}
const mhz = (hz: number) => (hz / 1e6).toFixed(3);

/** The station a QSY suggestion is being written to (B5), or "". */
const suggesting = ref("");
/** "Tune and listen" was pressed: the card stays until dismissed. */
const tuned = ref(false);
async function tune(i: number) {
  tuned.value = await tuneSuggested(i);
}
function dismiss() {
  extras.suggestion = null;
  tuned.value = false;
}

async function pick(h: HeardStation) {
  await openPeer(h.callsign);
  if (app.page !== "operate" && app.page !== "conversations") app.page = "operate";
  emit("opened", h.callsign);
}
</script>

<template>
  <div class="heard">
    <div class="tools">
      <input v-model="query" class="search heard-search" type="search" :aria-label="t('heard.searchAria')" :placeholder="t('heard.searchPlaceholder')" />
      <select v-model="filter" class="heard-filter" :aria-label="t('heard.filterAria')">
        <option v-for="f in FILTERS" :key="f" :value="f">{{ t(`heard.filter.${f}`) }}</option>
      </select>
    </div>
    <p v-if="discoveryError.star" class="err small" role="alert">{{ discoveryError.star }}</p>
    <section v-if="extras.suggestion" class="card stack small qsy-suggestion" role="status" aria-live="polite">
      <p>{{ t("qsy.suggested", { peer: extras.suggestion.from, freq: describeTuning(extras.suggestion.candidates[0]) }) }}</p>
      <p class="muted">{{ t("qsy.suggestionOnly") }}</p>
      <div class="row">
        <button v-for="(c, i) in extras.suggestion.candidates" :key="i" class="qsy-tune-suggested" :title="inSession ? t('rig.refused.connected') : ''" @click="tune(i)">
          {{ t("qsy.tuneAndListen", { freq: describeTuning(c) }) }}
        </button>
        <button class="quiet" @click="dismiss">{{ t("qsy.dismiss") }}</button>
      </div>
      <p v-if="tuned" class="ok">{{ t("qsy.tunedListening") }}</p>
      <p v-if="extras.error && !suggesting" class="err" role="alert">{{ extras.error }}</p>
    </section>
    <section v-if="suggesting" class="card stack small qsy-suggest">
      <p class="label">{{ t("qsy.suggestTitle", { peer: suggesting }) }}</p>
      <QsyPropose :to="suggesting" @done="suggesting = ''" />
      <p v-if="extras.error" class="err" role="alert">{{ extras.error }}</p>
    </section>
    <ul v-if="shown.length" class="list" :aria-label="t('heard.listAria')">
      <li
        v-for="h in shown"
        :key="h.callsign"
        :class="['row', { fresh: discovery.fresh[h.callsign], active: h.callsign === chat.open }]"
        :data-callsign="h.callsign"
      >
        <button class="pick" :aria-current="h.callsign === chat.open ? 'true' : undefined" @click="pick(h)">
          <Avatar :callsign="h.callsign" :live="h.callsign === linkPeer" :size="32" />
          <span class="line">
            <b class="mono call">{{ h.callsign }}</b>
            <span v-if="h.source" :class="['badge', h.source]">{{ t(`heard.source.${h.source}`) }}</span>
            <span class="grow"></span>
            <span v-if="h.lastHeardUtc" class="small muted mono age" :title="utc(h.lastHeardUtc)">{{ age(h.lastHeardUtc) }}</span>
            <span v-else class="small muted">{{ t("heard.notHeard") }}</span>
          </span>
          <span class="line small muted meta">
            <span v-if="h.grid" class="mono grid" :title="t('heard.gridTitle')">{{ h.grid }}</span>
            <span v-if="h.snr" class="mono snr" :title="provenance(h.snr)">{{ t("heard.snr", { db: h.snr.value.toFixed(1) }) }}</span>
            <span v-if="h.freq" class="mono freq" :title="provenance(h.freq)">{{ t("heard.mhz", { mhz: mhz(h.freq.value) }) }}</span>
            <span v-if="h.band" class="mono band">{{ h.band }}</span>
            <span v-if="h.offsetHz" class="mono offset" :title="provenance(h.offsetHz)">{{ t("heard.offset", { hz: h.offsetHz.value.toFixed(1) }) }}</span>
            <span v-if="h.count > 1" class="count">{{ t("heard.count", h.count) }}</span>
          </span>
          <span v-if="h.name || h.qth || h.status" class="line small beacon-fields" :title="t('heard.fromBeacon')">
            <span v-if="h.name" class="name">{{ h.name.value }}</span>
            <span v-if="h.qth" class="qth muted">{{ h.qth.value }}</span>
            <span v-if="h.status" class="status muted">“{{ h.status.value }}”</span>
          </span>
        </button>
        <button
          class="suggest-qsy quiet small"
          :aria-label="t('qsy.suggestAria', { callsign: h.callsign })"
          :title="t('qsy.suggestAria', { callsign: h.callsign })"
          :disabled="!activeProfile"
          @click="suggesting = h.callsign"
        >{{ t("qsy.suggest") }}</button>
        <button
          class="star quiet"
          :aria-pressed="h.favourite"
          :aria-label="t(h.favourite ? 'heard.unstar' : 'heard.star', { callsign: h.callsign })"
          :title="t(h.favourite ? 'heard.unstar' : 'heard.star', { callsign: h.callsign })"
          :disabled="!activeProfile"
          @click="setFavourite(h.callsign, !h.favourite)"
        >{{ h.favourite ? "★" : "☆" }}</button>
      </li>
    </ul>
    <p v-else-if="discovery.heard.length" class="muted small">{{ t("heard.noMatch") }}</p>
    <div v-else class="empty stack">
      <p class="muted small">{{ t("sidebar.heardEmpty") }}</p>
      <p class="muted small note">{{ t("heard.notReachable") }}</p>
    </div>
    <p class="sr-only" role="status" aria-live="polite">{{ discovery.announce }}</p>
  </div>
</template>

<style scoped>
.heard { display: grid; gap: var(--space); align-content: start; padding: calc(var(--space) * 1.5); overflow-y: auto; min-height: 0; height: 100%; }
.tools { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px; }
.search { width: 100%; min-width: 0; }
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
.row {
  display: grid; grid-template-columns: minmax(0, 1fr) auto auto; align-items: start; gap: 2px;
  border: 1px solid transparent; border-radius: 9px;
}
.row.active { border-color: color-mix(in srgb, var(--accent) 60%, var(--border)); background: var(--raised); }
/* A station just heard: a clear highlight that fades over about 3 s. Under
   reduced motion there is no animation (tokens.css) and the highlight stays static. */
.row.fresh { background: color-mix(in srgb, var(--accent) 18%, var(--raised)); border-color: var(--accent); animation: heard-fade 3s ease-out forwards; }
@keyframes heard-fade {
  from { background: color-mix(in srgb, var(--accent) 30%, var(--raised)); border-color: var(--accent); }
  to { background: transparent; border-color: transparent; }
}
.pick { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 2px 10px; text-align: left; padding: 6px 8px; border-color: transparent; background: transparent; min-width: 0; }
.pick > .avatar { grid-row: 1 / span 3; align-self: start; margin-top: 2px; }
.pick > .line { grid-column: 2; }
.line { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; min-width: 0; }
.call { overflow-wrap: anywhere; }
.meta { gap: 2px 8px; }
.meta > span { white-space: nowrap; }
.suggest-qsy { min-height: 0; padding: 4px 6px; border-color: transparent; }
.beacon-fields { gap: 2px 8px; }
.star { min-height: 0; padding: 4px 8px; border-color: transparent; font-size: calc(16px * var(--font-scale)); line-height: 1; color: var(--warning); }
.star[aria-pressed="false"] { color: var(--text-2); }
.note { border-top: 1px solid var(--border); padding-top: var(--space); }
.empty { padding: calc(var(--space) * 0.5); }
</style>

