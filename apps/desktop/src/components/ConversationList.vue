<script setup lang="ts">
// Conversations with search, plus a callsign entry to open a new one.
// Heard stations (discovery) are not part of M2 and are not shown.
import { computed, ref } from "vue";
import { activeProfile, chat, linkPeer, openPeer } from "../store";

const emit = defineEmits<{ opened: [peer: string] }>();
const query = ref("");
const entry = ref("");

const shown = computed(() => {
  const q = query.value.trim().toUpperCase();
  return q ? chat.conversations.filter((c) => c.peer.includes(q)) : chat.conversations;
});
const ago = (utc: string) => utc.slice(5, 10) + " " + utc.slice(11, 16) + " UTC";

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
    <div class="row head">
      <h2>Conversations</h2>
      <span class="pill">{{ chat.conversations.length }}</span>
    </div>
    <form class="stack" @submit.prevent="openEntry">
      <label>
        Callsign
        <input
          v-model="entry"
          class="mono peer"
          maxlength="15"
          autocomplete="off"
          spellcheck="false"
          :disabled="!activeProfile"
          placeholder="e.g. ZS1ABC"
          @change="openEntry"
        />
      </label>
    </form>
    <input v-model="query" class="search" type="search" aria-label="Search conversations" placeholder="Search callsign" />
    <ul class="list" aria-label="Conversations">
      <li v-for="c in shown" :key="c.peer">
        <button :class="['item', { active: c.peer === chat.open }]" :aria-current="c.peer === chat.open ? 'true' : undefined" @click="open(c.peer)">
          <span class="row top">
            <b class="mono">{{ c.peer }}</b>
            <span v-if="c.peer === linkPeer" class="small ok">on the link</span>
            <span v-else-if="chat.unread[c.peer]" class="small accent">{{ chat.unread[c.peer] }} new</span>
          </span>
          <span class="meta small muted">
            <span>{{ c.messages }} message{{ c.messages === 1 ? "" : "s" }}</span>
            <span v-if="c.uncertain" class="warn">{{ c.uncertain }} uncertain</span>
            <span v-if="c.draft">draft</span>
            <span class="grow"></span>
            <span v-if="c.messages" class="mono">{{ ago(c.lastUtc) }}</span>
          </span>
        </button>
      </li>
    </ul>
    <p v-if="!shown.length" class="muted small">{{ query ? "No callsign matches." : "No conversations yet. Type a callsign to start one." }}</p>
    <p class="note small muted">Stations heard on the air are not listed yet. A conversation here does not mean the station is reachable now.</p>
  </div>
</template>

<style scoped>
.conv { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; min-height: 0; height: 100%; }
.head { justify-content: space-between; }
.search { width: 100%; }
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
.item { width: 100%; text-align: left; display: grid; gap: 4px; padding: 8px 10px; border-color: transparent; background: transparent; }
.item.active { border-color: color-mix(in srgb, var(--accent) 60%, var(--border)); background: var(--raised); }
.top { justify-content: space-between; }
.meta { display: flex; flex-wrap: wrap; gap: 2px 8px; }
.meta > span { white-space: nowrap; }
.accent { color: var(--accent); }
.note { border-top: 1px solid var(--border); padding-top: var(--space); }
</style>
