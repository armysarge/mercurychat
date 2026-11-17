<script setup lang="ts">
// The sidebar's Chats tab: search, a callsign entry to open a new conversation, and the list.
// The search box filters the list by callsign and, from two characters, also
// searches every conversation's messages by text or callsign (CHT-05).
import { computed, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import type { ChatMessage } from "../bindings/ChatMessage";
import { activeProfile, chat, linkPeer, openPeer, searchHistory } from "../store";

const { t } = useI18n();
const emit = defineEmits<{ opened: [peer: string] }>();
const query = ref("");
const entry = ref("");

const shown = computed(() => {
  const q = query.value.trim().toUpperCase();
  return q ? chat.conversations.filter((c) => c.peer.includes(q)) : chat.conversations;
});
const ago = (utc: string) => utc.slice(5, 10) + " " + utc.slice(11, 16) + " UTC";

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
    <form class="stack" @submit.prevent="openEntry">
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
    <input v-model="query" class="search" type="search" :aria-label="t('conversations.searchAria')" :placeholder="t('conversations.searchPlaceholder')" />
    <ul class="list" :aria-label="t('conversations.listAria')">
      <li v-for="c in shown" :key="c.peer">
        <button :class="['item', { active: c.peer === chat.open }]" :aria-current="c.peer === chat.open ? 'true' : undefined" @click="open(c.peer)">
          <span class="row top">
            <b class="mono">{{ c.peer }}</b>
            <span v-if="c.peer === linkPeer" class="small ok">{{ t("conversations.onLink") }}</span>
            <span v-else-if="chat.unread[c.peer]" class="small accent">{{ t("conversations.newCount", chat.unread[c.peer]) }}</span>
          </span>
          <span class="meta small muted">
            <span>{{ t("conversations.messageCount", c.messages) }}</span>
            <span v-if="c.uncertain" class="warn">{{ t("conversations.uncertain", c.uncertain) }}</span>
            <span v-if="c.draft">{{ t("conversations.draft") }}</span>
            <span class="grow"></span>
            <span v-if="c.messages" class="mono">{{ ago(c.lastUtc) }}</span>
          </span>
        </button>
      </li>
    </ul>
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
    <p v-if="!shown.length" class="muted small">{{ query ? t("conversations.noMatch") : t("conversations.none") }}</p>
    <p class="note small muted">{{ t("conversations.note") }}</p>
  </div>
</template>

<style scoped>
.conv { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; min-height: 0; height: 100%; }
.search { width: 100%; }
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
.item { width: 100%; text-align: left; display: grid; gap: 4px; padding: 8px 10px; border-color: transparent; background: transparent; }
.item.active { border-color: color-mix(in srgb, var(--accent) 60%, var(--border)); background: var(--raised); }
.top { justify-content: space-between; }
.meta { display: flex; flex-wrap: wrap; gap: 2px 8px; }
.meta > span { white-space: nowrap; }
.accent { color: var(--accent); }
.search-hits { display: grid; gap: 4px; border-top: 1px solid var(--border); padding-top: var(--space); }
.hit { overflow-wrap: anywhere; }
.note { border-top: 1px solid var(--border); padding-top: var(--space); }
</style>
