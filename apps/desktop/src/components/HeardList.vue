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
import { activeProfile, app, chat, discovery, discoveryError, openPeer, setFavourite } from "../store";

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
    <ul v-if="shown.length" class="list" :aria-label="t('heard.listAria')">
      <li
        v-for="h in shown"
        :key="h.callsign"
        :class="['row', { fresh: discovery.fresh[h.callsign], active: h.callsign === chat.open }]"
        :data-callsign="h.callsign"
      >
        <button class="pick" :aria-current="h.callsign === chat.open ? 'true' : undefined" @click="pick(h)">
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
        </button>
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
  display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: start; gap: 2px;
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
.pick { display: grid; gap: 2px; text-align: left; padding: 6px 8px; border-color: transparent; background: transparent; min-width: 0; }
.line { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; min-width: 0; }
.call { overflow-wrap: anywhere; }
.meta { gap: 2px 8px; }
.meta > span { white-space: nowrap; }
.star { min-height: 0; padding: 4px 8px; border-color: transparent; font-size: calc(16px * var(--font-scale)); line-height: 1; color: var(--warning); }
.star[aria-pressed="false"] { color: var(--text-2); }
.note { border-top: 1px solid var(--border); padding-top: var(--space); }
.empty { padding: calc(var(--space) * 0.5); }
</style>

