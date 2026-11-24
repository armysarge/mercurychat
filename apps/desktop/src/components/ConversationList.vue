<script setup lang="ts">
// The sidebar's Chats tab, in the design canvas's form (M5 Task 11c): the
// title, search, the All / Sessions / CQ / Beacons chips, "Live now" (the
// station on the link) above the rest, each with its last message, and Call
// CQ / Beacon now at the foot. A callsign entry opens a new conversation.
// The search box filters the list by callsign and, from two characters, also
// searches every conversation's messages by text or callsign (CHT-05).
import { computed, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import type { ChatMessage } from "../bindings/ChatMessage";
import type { Conversation } from "../bindings/Conversation";
import type { HeardSource } from "../bindings/HeardSource";
import {
  activeProfile, chat, discovery, discoveryError, linkPeer, modemState, openPeer, searchHistory, sendBeaconNow, sendCq, session,
} from "../store";
import Avatar from "./Avatar.vue";
import { openAt, openFrom } from "../stationMenu";

const { t } = useI18n();
const emit = defineEmits<{ opened: [peer: string] }>();
const query = ref("");
const entry = ref("");

/** The chips filter by how the station was last heard (Heard list). */
const CHIPS: { id: "all" | HeardSource; label: string }[] = [
  { id: "all", label: "conversations.chipAll" },
  { id: "session", label: "conversations.chipSessions" },
  { id: "cq", label: "conversations.chipCq" },
  { id: "beacon", label: "conversations.chipBeacons" },
];
const chip = ref<"all" | HeardSource>("all");
const heardOf = (peer: string) => discovery.heard.find((h) => h.callsign === peer);

const shown = computed(() => {
  const q = query.value.trim().toUpperCase();
  return chat.conversations.filter((c) => (!q || c.peer.includes(q)) && (chip.value === "all" || heardOf(c.peer)?.source === chip.value));
});
/** The station on the link (or being called) first, under "Live now". */
const livePeer = computed(() => session.value?.peer ?? linkPeer.value ?? "");
const sections = computed(() => {
  const live = shown.value.filter((c) => c.peer === livePeer.value);
  const rest = shown.value.filter((c) => c.peer !== livePeer.value);
  return [
    { id: "live-now", title: live.length ? "conversations.liveNow" : "", rows: live },
    { id: "recent", title: live.length && rest.length ? "conversations.recent" : "", rows: rest },
  ].filter((x) => x.rows.length);
});
const ago = (utc: string) => utc.slice(5, 10) + " " + utc.slice(11, 16) + " UTC";
/** "11:03" today, else "11-23": the full time is on hover. */
function short(utc: string): string {
  return utc.slice(0, 10) === new Date().toISOString().slice(0, 10) ? utc.slice(11, 16) : utc.slice(5, 10);
}
/** The row's second line: the last message on one line ("You: ..." when ours), else the draft. */
function preview(c: Conversation): string {
  if (c.lastBody) return (c.lastOut ? t("transcript.you") + ": " : "") + c.lastBody.replace(/\s+/g, " ");
  return c.draft ? t("conversations.draft") + ": " + c.draft.replace(/\s+/g, " ") : "";
}
const mhz = (hz: number) => (hz / 1e6).toFixed(3);

/** Call CQ / Beacon now: only while the modem is idle; the reason otherwise. */
const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
const why = computed(() => {
  if (!activeProfile.value || idle.value) return "";
  return livePeer.value ? t("conversations.footBusy", { peer: livePeer.value }) : t("strip.idleOnly");
});

/** Messages matching the search, newest first (two characters or more). */
const hits = ref<ChatMessage[]>([]);
const searchError = ref("");
let searchTimer: ReturnType<typeof setTimeout> | undefined;
watch(query, (q) => {
  clearTimeout(searchTimer);
  const wanted = q.trim();
  if (wanted.length < 2 || !activeProfile.value) {
    hits.value = [];
    return;
  }
  searchTimer = setTimeout(async () => {
    try {
      const found = await searchHistory(wanted);
      if (query.value.trim() === wanted) {
        hits.value = found;
        searchError.value = "";
      }
    } catch (e) {
      searchError.value = describeError(e);
    }
  }, 250);
});
onUnmounted(() => clearTimeout(searchTimer));
/** About 60 characters of the message around the match, on one line. */
function snippet(body: string): string {
  const flat = body.replace(/\s+/g, " ");
  const i = flat.toLowerCase().indexOf(query.value.trim().toLowerCase());
  const from = Math.max(0, i - 20);
  return (from > 0 ? "…" : "") + flat.slice(from, from + 60) + (flat.length > from + 60 ? "…" : "");
}

async function open(peer: string) {
  await openPeer(peer);
  emit("opened", peer);
}
function openEntry() {
  const p = entry.value.trim();
  if (p) void open(p);
}
</script>

<template>
  <div class="conv">
    <h2 class="list-title">{{ t("sidebar.chats") }}</h2>
    <label class="search-box">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm9 16-4-4" /></svg>
      <input v-model="query" class="search" type="search" :aria-label="t('conversations.searchAria')" :placeholder="t('conversations.searchPlaceholder')" />
    </label>
    <div class="chips" role="group" :aria-label="t('conversations.chipsAria')">
      <button v-for="c in CHIPS" :key="c.id" type="button" class="chip" :aria-pressed="chip === c.id" @click="chip = c.id">{{ t(c.label) }}</button>
    </div>
    <form class="new-chat" @submit.prevent="openEntry">
      <label>
        {{ t("conversations.callsign") }}
        <input
          v-model="entry"
          class="mono peer"
          maxlength="15"
          autocomplete="off"
          spellcheck="false"
          :disabled="!activeProfile"
          :placeholder="t('conversations.callsignExample')"
          @change="openEntry"
        />
      </label>
    </form>
    <section v-for="sec in sections" :key="sec.id" :class="['section', sec.id]">
      <h3 v-if="sec.title" class="label">{{ t(sec.title) }}</h3>
      <ul class="list" :aria-label="sec.title ? t(sec.title) : t('conversations.listAria')">
        <li v-for="c in sec.rows" :key="c.peer" class="conv-row" :data-callsign="c.peer" @contextmenu="openAt($event, c.peer, 'chats')">
          <button :class="['item', 'with-avatar', { active: c.peer === chat.open }]" :aria-current="c.peer === chat.open ? 'true' : undefined" @click="open(c.peer)">
            <Avatar :callsign="c.peer" :live="c.peer === linkPeer" :size="40" />
            <span class="row top">
              <b class="mono">{{ c.peer }}</b>
              <span v-if="c.peer === linkPeer" class="small ok">{{ t("conversations.onLink") }}</span>
              <span v-else-if="chat.unread[c.peer]" class="small accent unread">{{ t("conversations.newCount", chat.unread[c.peer]) }}</span>
              <span class="grow"></span>
              <span v-if="c.messages" class="mono small muted when" :title="ago(c.lastUtc)">{{ short(c.lastUtc) }}</span>
            </span>
            <span v-if="preview(c)" class="preview small">{{ preview(c) }}</span>
            <span class="meta small muted">
              <span>{{ t("conversations.messageCount", c.messages) }}</span>
              <span v-if="c.uncertain" class="warn">{{ t("conversations.uncertain", c.uncertain) }}</span>
              <span v-if="c.draft && c.lastBody">{{ t("conversations.draft") }}</span>
              <template v-if="heardOf(c.peer)">
                <span v-if="heardOf(c.peer)?.freq" class="mono">{{ mhz(heardOf(c.peer)?.freq?.value ?? 0) }}</span>
                <span v-if="heardOf(c.peer)?.band" class="mono">{{ heardOf(c.peer)?.band }}</span>
                <span>{{ t("conversations.heardTimes", heardOf(c.peer)?.count ?? 0) }}</span>
              </template>
            </span>
          </button>
          <button
            class="more quiet"
            aria-haspopup="menu"
            :aria-label="t('stationMenu.more', { callsign: c.peer })"
            :title="t('stationMenu.more', { callsign: c.peer })"
            @click="openFrom($event.currentTarget as HTMLElement, c.peer, 'chats')"
          >…</button>
        </li>
      </ul>
    </section>
    <section v-if="hits.length" class="search-hits" :aria-label="t('conversations.hitsAria')">
      <span class="label">{{ t("conversations.hitsTitle", hits.length) }}</span>
      <ul class="list">
        <li v-for="m in hits" :key="m.direction + m.id">
          <button class="hit item" @click="open(m.peer)">
            <span class="row top">
              <b class="mono">{{ m.peer }}</b>
              <span class="small muted mono">{{ ago(m.receivedUtc ?? m.createdUtc) }}</span>
            </span>
            <span class="small">{{ m.direction === "out" ? t("transcript.you") + ": " : "" }}{{ snippet(m.body) }}</span>
          </button>
        </li>
      </ul>
    </section>
    <p v-if="searchError" class="err small" role="alert">{{ searchError }}</p>
    <p v-if="!shown.length" class="muted small">{{ query || chip !== "all" ? t("conversations.noMatch") : t("conversations.none") }}</p>
    <p class="note small muted">{{ t("conversations.note") }}</p>
    <div class="foot">
      <div class="foot-buttons">
        <button class="foot-cq" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendCq">{{ t("conversations.footCq") }}</button>
        <button class="foot-beacon" :disabled="!activeProfile || !idle" :title="t('strip.idleOnly')" @click="sendBeaconNow">{{ t("conversations.footBeacon") }}</button>
      </div>
      <p v-if="why" class="why small muted">{{ why }}</p>
      <p v-if="discoveryError.cq || discoveryError.beacon" class="err small" role="alert">{{ discoveryError.cq || discoveryError.beacon }}</p>
    </div>
  </div>
</template>

<style scoped>
.conv { display: grid; gap: calc(var(--space) * 1.25); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; min-height: 0; height: 100%; }
.list-title { margin: 0; font-size: calc(20px * var(--font-scale)); font-weight: 700; }
.search-box { position: relative; display: block; }
.search-box svg { position: absolute; left: 10px; top: 50%; width: 16px; height: 16px; transform: translateY(-50%); fill: none; stroke: var(--text-2); stroke-width: 1.8; stroke-linecap: round; pointer-events: none; }
.search { width: 100%; padding-left: 32px; border-radius: 10px; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.chip { min-height: 0; padding: 4px 12px; border-radius: 999px; font-size: calc(12px * var(--font-scale)); }
.chip[aria-pressed="true"] { background: var(--accent); border-color: var(--accent); color: var(--accent-text); }
.new-chat label { display: grid; gap: 4px; font-size: calc(12px * var(--font-scale)); color: var(--text-2); }
.section { display: grid; gap: 4px; }
.section .label { margin: 0; font-size: calc(11px * var(--font-scale)); letter-spacing: 0.06em; text-transform: uppercase; color: var(--text-2); }
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
.item { width: 100%; text-align: left; display: grid; gap: 2px; padding: 8px 10px; border-color: transparent; background: transparent; border-radius: 12px; }
.item.with-avatar { grid-template-columns: auto minmax(0, 1fr); column-gap: 12px; }
.with-avatar > .avatar { grid-row: 1 / span 3; align-self: center; }
.with-avatar > .row, .with-avatar > .meta, .with-avatar > .preview { grid-column: 2; }
.item.active { border-color: color-mix(in srgb, var(--accent) 60%, var(--border)); background: var(--raised); }
.conv-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: 2px; }
.more { min-height: 0; padding: 4px 8px; border-color: transparent; font-size: calc(16px * var(--font-scale)); line-height: 1; color: var(--text-2); }
.top { gap: 6px; }
.preview { color: var(--text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.meta { display: flex; flex-wrap: wrap; gap: 2px 8px; }
.meta > span { white-space: nowrap; }
.accent { color: var(--accent); }
.search-hits { display: grid; gap: 4px; border-top: 1px solid var(--border); padding-top: var(--space); }
.hit { overflow-wrap: anywhere; }
.note { border-top: 1px solid var(--border); padding-top: var(--space); }
.foot { display: grid; gap: 6px; border-top: 1px solid var(--border); padding-top: var(--space); }
.foot-buttons { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.why { margin: 0; }
</style>
