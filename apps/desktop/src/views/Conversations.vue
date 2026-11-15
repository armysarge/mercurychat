<script setup lang="ts">
// Conversations (docs/04 §4): callsign list with search and the chosen
// peer's history, readable whether or not a link is up. Ctrl+F searches it.
import { onMounted, onUnmounted, ref } from "vue";
import ConversationList from "../components/ConversationList.vue";
import Transcript from "../components/Transcript.vue";
import { chat } from "../store";

const search = ref("");
const box = ref<HTMLInputElement | null>(null);

function onKey(e: KeyboardEvent) {
  if (e.ctrlKey && (e.key === "f" || e.key === "F")) {
    e.preventDefault();
    box.value?.focus();
  }
}
onMounted(() => document.addEventListener("keydown", onKey));
onUnmounted(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="conversations">
    <aside class="list-pane"><ConversationList /></aside>
    <section class="history" aria-label="History">
      <div class="row head">
        <h1 class="mono">{{ chat.open || "History" }}</h1>
        <span class="grow"></span>
        <input ref="box" v-model="search" type="search" class="history-search" aria-label="Search this history (Ctrl+F)" placeholder="Search history (Ctrl+F)" />
      </div>
      <Transcript class="fill" :filter="search" />
    </section>
  </div>
</template>

<style scoped>
.conversations { display: grid; grid-template-columns: 280px minmax(0, 1fr); height: 100%; min-height: 0; }
.list-pane { border-right: 1px solid var(--border); min-height: 0; }
.history { display: flex; flex-direction: column; min-height: 0; }
.head { padding: calc(var(--space) * 1.5) calc(var(--space) * 2); border-bottom: 1px solid var(--border); }
.head h1 { font-size: 18px; }
.fill { flex: 1; }
@media (max-width: 900px) {
  .conversations { grid-template-columns: minmax(0, 1fr); grid-template-rows: auto minmax(0, 1fr); }
  .list-pane { border-right: 0; border-bottom: 1px solid var(--border); max-height: 40vh; }
}
</style>
